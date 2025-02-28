//! Interacts with the local Docker daemon.

use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use bollard::{
    container::Config,
    image::{CreateImageOptions, ListImagesOptions},
    Docker,
};
use bytes::Bytes;
use color_eyre::eyre::{Context, Result};
use derive_more::Debug;
use futures_lite::{Stream, StreamExt};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio_tar::Archive;
use tracing::{debug, info, warn};

use crate::{
    transform::Chunk, Digest, FilterMatch, Filters, ImageSource, LayerDescriptor, LayerMediaType,
    Platform, Reference,
};

/// Each instance represents a Docker daemon connection for a specific image.
/// Similar to Registry, this follows the builder pattern for creating instances.
#[derive(Debug, Clone)]
pub struct Daemon {
    /// The reference to the image in the daemon
    pub reference: Reference,

    /// The Docker client for interacting with the daemon
    #[debug(skip)]
    docker: Docker,

    /// Layer filters.
    /// Layers that match any filter are included in the set of layers processed.
    layer_filters: Filters,

    /// File filters.
    /// Files that match any filter are included in the set of files processed.
    file_filters: Filters,

    /// The platform to use for the daemon.
    platform: Option<Platform>,
}

#[bon::bon]
impl Daemon {
    /// Create a new Daemon instance with the specified parameters.
    #[builder]
    pub async fn new(
        /// The reference to the image in the daemon
        reference: Reference,

        /// Filters for layers.
        /// Layers that match any filter are included in the set of layers processed.
        layer_filters: Option<Filters>,

        /// Filters for files.
        /// Files that match any filter are included in the set of files processed.
        file_filters: Option<Filters>,

        /// The platform to use for the daemon.
        platform: Option<Platform>,
    ) -> Result<Self> {
        let docker = Docker::connect_with_local_defaults().context("connect to Docker daemon")?;

        // Verify Docker daemon is accessible
        docker
            .version()
            .await
            .context("verify Docker daemon connection")?;

        Ok(Self {
            reference,
            docker,
            layer_filters: layer_filters.unwrap_or_default(),
            file_filters: file_filters.unwrap_or_default(),
            platform,
        })
    }
}

impl Daemon {
    /// List all images in the Docker daemon.
    pub async fn list_images(&self) -> Result<Vec<String>> {
        let options = Some(ListImagesOptions::<String> {
            all: true,
            ..Default::default()
        });

        let images = self
            .docker
            .list_images(options)
            .await
            .context("list images")?;

        let mut image_tags = Vec::new();
        for image in images {
            // RepoTags is a Vec, not an Option
            image_tags.extend(image.repo_tags);
        }

        Ok(image_tags)
    }

    /// Checks if an image exists in the Docker daemon.
    pub async fn image_exists(&self) -> Result<bool> {
        let image_name = self.reference.to_string();
        let options = Some(ListImagesOptions::<String> {
            all: true,
            ..Default::default()
        });

        let images = self
            .docker
            .list_images(options)
            .await
            .context("list images")?;

        for image in images {
            if image.repo_tags.iter().any(|tag| tag == &image_name) {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Pull an image if it doesn't exist in the Docker daemon.
    async fn ensure_image(&self) -> Result<()> {
        // Check if image exists
        if self.image_exists().await? {
            info!("Image {} already exists in Docker daemon", self.reference);
            return Ok(());
        }

        info!("Pulling image {} from registry", self.reference);

        let tag = match &self.reference.version {
            crate::Version::Tag(tag) => tag.clone(),
            crate::Version::Digest(_) => {
                warn!("Using digest for Docker daemon pull is not supported, falling back to 'latest'");
                "latest".to_string()
            }
        };

        let mut options = CreateImageOptions {
            from_image: self.reference.repository.clone(),
            tag,
            ..Default::default()
        };

        // Apply platform if specified
        if let Some(platform) = &self.platform {
            info!("Requesting image for platform: {}", platform);
            options.platform = platform.to_string();
        }

        let options = Some(options);

        let mut pull_stream = self.docker.create_image(options, None, None);

        while let Some(info) = pull_stream.next().await {
            match info {
                Ok(info) => debug!(?info, "Pull progress"),
                Err(e) => warn!(?e, "Error during pull"),
            }
        }

        Ok(())
    }

    /// Export an image to a temporary directory and return the path to the tarball.
    async fn export_image(&self) -> Result<(TempDir, PathBuf)> {
        // Ensure the image exists
        self.ensure_image().await?;

        // Create a temporary directory for the export
        let temp_dir = tempfile::TempDir::new().context("create temporary directory")?;
        let export_path = temp_dir.path().join("image.tar");

        // Create a container from the image (we don't need to run it)
        let container_config = Config {
            image: Some(self.reference.to_string()),
            cmd: Some(vec!["true".to_string()]),
            ..Default::default()
        };

        // Platform is applied at image pull time, not container creation time
        // We've already applied platform constraints during the ensure_image step

        let container = self
            .docker
            .create_container::<String, String>(None, container_config)
            .await
            .context("create temporary container")?;

        // Export the container to a tar file
        let export_stream = self.docker.export_container(&container.id);

        // Write the export stream to a file
        let mut file = tokio::fs::File::create(&export_path)
            .await
            .context("create export file")?;

        let mut export_stream = Box::pin(export_stream);
        while let Some(chunk) = export_stream.next().await {
            let chunk = chunk.context("read export chunk")?;
            file.write_all(&chunk).await.context("write export chunk")?;
        }

        // Remove the temporary container
        self.docker
            .remove_container(&container.id, None)
            .await
            .context("remove temporary container")?;

        Ok((temp_dir, export_path))
    }

    /// Enumerate layers for a Docker image.
    /// Layers are returned in order from the base image to the application.
    pub async fn layers(&self) -> Result<Vec<LayerDescriptor>> {
        // This is a simplified implementation as we don't have the same concept of layers
        // as in the OCI registry. Instead, we'll generate layer descriptors based on the
        // contents of the exported tar file.

        // Export the image to a tarball
        let (_temp_dir, export_path) = self.export_image().await?;

        // Read the tarball to get the layers
        let tar_data = tokio::fs::read(&export_path)
            .await
            .context("read export tar")?;

        // We're going to simulate layers based on the tar entries
        // This is a simplified approach - in a real implementation, we'd parse the manifest.json
        // from the exported tarball to get the actual layers

        // For now, we'll just return a single "layer" that represents the entire exported image
        let digest_hex = blake3::hash(&tar_data).to_hex().to_string();
        let digest_str = format!("sha256:{}", digest_hex);
        let digest = Digest::from_str(&digest_str).context("parse digest")?;

        let layer = LayerDescriptor::builder()
            .digest(digest)
            .size(tar_data.len() as i64)
            .media_type(LayerMediaType::Oci(vec![]))
            .build();

        if self.layer_filters.matches(&layer) {
            Ok(vec![layer])
        } else {
            Ok(vec![])
        }
    }

    /// Pull a layer from the Docker daemon and return it as a stream.
    pub async fn pull_layer(
        &self,
        layer: &LayerDescriptor,
    ) -> Result<impl Stream<Item = Result<Bytes>>> {
        self.pull_layer_internal(layer)
            .await
            .map(|stream| stream.map(|chunk| chunk.context("read chunk")))
    }

    async fn pull_layer_internal(
        &self,
        _layer: &LayerDescriptor,
    ) -> Result<impl Stream<Item = Chunk>> {
        // Export the image to a tarball
        let (_temp_dir, export_path) = self.export_image().await?;

        // Read the tarball
        let tar_data = tokio::fs::read(&export_path)
            .await
            .context("read export tar")?;

        // Create a stream from the tar data
        let stream = futures_lite::stream::once(Ok(Bytes::from(tar_data)) as Chunk);

        Ok(stream)
    }

    /// List files in a layer.
    pub async fn list_files(&self, layer: &LayerDescriptor) -> Result<Vec<String>> {
        let stream = self.pull_layer_internal(layer).await?;
        let reader = tokio_util::io::StreamReader::new(stream);
        let mut archive = Archive::new(reader);
        let mut entries = archive.entries().context("read entries from tar")?;

        let mut files = Vec::new();
        while let Some(entry) = entries.next().await {
            let entry = entry.context("read entry")?;
            let path = entry.path().context("read entry path")?;

            // Apply file filters if they exist
            if !self.file_filters.matches(&path.to_path_buf()) {
                debug!(?path, "skip: path filter");
                continue;
            }

            debug!(?path, "enumerate");
            files.push(path.to_string_lossy().to_string());
        }

        Ok(files)
    }

    /// Apply a layer to a location on disk.
    pub async fn apply_layer(&self, layer: &LayerDescriptor, output: &Path) -> Result<()> {
        let stream = self.pull_layer_internal(layer).await?;
        let reader = tokio_util::io::StreamReader::new(stream);
        let mut archive = Archive::new(reader);

        // Just unpack the whole archive - we'll let tokio_tar handle the details
        // This is a simplification, but should work for our purposes
        archive.unpack(output).await.context("unpack archive")?;

        debug!("Applied layer to {}", output.display());

        Ok(())
    }
}

#[async_trait::async_trait]
impl ImageSource for Daemon {
    /// Enumerate layers for a Docker image.
    /// Layers are returned in order from the base image to the application.
    #[tracing::instrument(skip(self))]
    async fn layers(&self) -> Result<Vec<LayerDescriptor>> {
        // This is a simplified implementation as we don't have the same concept of layers
        // as in the OCI registry. Instead, we'll generate layer descriptors based on the
        // contents of the exported tar file.

        // Export the image to a tarball
        let (_temp_dir, export_path) = self.export_image().await?;

        // Read the tarball to get the layers
        let tar_data = tokio::fs::read(&export_path)
            .await
            .context("read export tar")?;

        // We're going to simulate layers based on the tar entries
        // This is a simplified approach - in a real implementation, we'd parse the manifest.json
        // from the exported tarball to get the actual layers

        // For now, we'll just return a single "layer" that represents the entire exported image
        let digest_hex = blake3::hash(&tar_data).to_hex().to_string();
        let digest_str = format!("sha256:{}", digest_hex);
        let digest = Digest::from_str(&digest_str).context("parse digest")?;

        let layer = LayerDescriptor::builder()
            .digest(digest)
            .size(tar_data.len() as i64)
            .media_type(LayerMediaType::Oci(vec![]))
            .build();

        if self.layer_filters.matches(&layer) {
            Ok(vec![layer])
        } else {
            Ok(vec![])
        }
    }

    /// List files in a layer.
    #[tracing::instrument(skip(self))]
    async fn list_files(&self, layer: &LayerDescriptor) -> Result<Vec<String>> {
        let stream = self.pull_layer_internal(layer).await?;
        let reader = tokio_util::io::StreamReader::new(stream);
        let mut archive = Archive::new(reader);
        let mut entries = archive.entries().context("read entries from tar")?;

        let mut files = Vec::new();
        while let Some(entry) = entries.next().await {
            let entry = entry.context("read entry")?;
            let path = entry.path().context("read entry path")?;

            // Apply file filters if they exist
            if !self.file_filters.matches(&path.to_path_buf()) {
                debug!(?path, "skip: path filter");
                continue;
            }

            debug!(?path, "enumerate");
            files.push(path.to_string_lossy().to_string());
        }

        Ok(files)
    }

    /// Apply a layer to a location on disk.
    #[tracing::instrument(skip(self))]
    async fn apply_layer(&self, layer: &LayerDescriptor, output: &Path) -> Result<()> {
        let stream = self.pull_layer_internal(layer).await?;
        let reader = tokio_util::io::StreamReader::new(stream);
        let mut archive = Archive::new(reader);

        // Just unpack the whole archive - we'll let tokio_tar handle the details
        // This is a simplification, but should work for our purposes
        archive.unpack(output).await.context("unpack archive")?;

        debug!("Applied layer to {}", output.display());

        Ok(())
    }
}

/// Checks if Docker daemon is available.
pub async fn is_daemon_available() -> bool {
    match Docker::connect_with_local_defaults() {
        Ok(docker) => docker.version().await.is_ok(),
        Err(_) => false,
    }
}
