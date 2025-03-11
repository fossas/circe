use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    pin::Pin,
    process::Stdio,
};

use crate::{
    cfs::{self, apply_tarball, collect_tmp, enumerate_tarball},
    homedir,
    transform::{self, Chunk},
    Authentication, Digest, FilterMatch, Filters, Layer, LayerMediaType, LayerMediaTypeFlag,
    Reference, Source,
};
use async_tempfile::TempFile;
use base64::Engine;
use bollard::Docker;
use bytes::Bytes;
use color_eyre::{
    eyre::{eyre, Context, OptionExt, Result},
    Section, SectionExt,
};
use derive_more::Debug;
use futures_lite::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use tap::{Pipe, TapFallible};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tar::Archive;
use tokio_util::io::ReaderStream;
use tracing::{debug, info, warn};

impl Authentication {
    /// Read authentication information for the host from the configured Docker credentials, if any.
    ///
    /// Reference:
    /// - https://docs.docker.com/reference/cli/docker/login
    /// - https://github.com/docker/docker-credential-helpers
    pub async fn docker(target: &Reference) -> Result<Self> {
        match Self::docker_internal(target).await {
            Ok(auth) => {
                debug!("inferred docker auth: {auth:?}");
                Ok(auth)
            }
            Err(err) => {
                warn!(?err, "unable to infer docker auth; trying unauthenticated");
                Ok(Authentication::None)
            }
        }
    }

    async fn docker_internal(target: &Reference) -> Result<Self> {
        let host = &target.host;
        let path = homedir()
            .context("get home directory")?
            .join(".docker")
            .join("config.json");

        let config = tokio::fs::read_to_string(&path)
            .await
            .context("read docker config")
            .with_section(|| path.display().to_string().header("Config file path:"))?;

        serde_json::from_str::<DockerConfig>(&config)
            .context("parse docker config")
            .with_section(|| path.display().to_string().header("Config file path:"))
            .with_section(|| config.header("Config file content:"))?
            .auth(host)
            .await
            .tap_ok(|auth| info!("inferred docker auth: {auth:?}"))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DockerConfig {
    /// The default credential store.
    ///
    /// The value of the config property is the suffix of the program to use (i.e. everything after `docker-credential-`).
    creds_store: Option<String>,

    /// Credential stores per host.
    ///
    /// Credential helpers are specified in a similar way to credsStore, but allow for multiple helpers to be configured at a time.
    /// Keys specify the registry domain, and values specify the suffix of the program to use (i.e. everything after docker-credential-).
    #[serde(default)]
    cred_helpers: HashMap<String, String>,

    /// Logged in hosts.
    #[serde(default)]
    auths: HashMap<String, DockerAuth>,
}

impl DockerConfig {
    /// Some hosts have fallback keys.
    /// Given a host, this function returns an iterator representing fallback keys to check for authentication.
    fn auth_keys(host: &str) -> impl Iterator<Item = &str> {
        if host == "docker.io" {
            vec!["docker.io", "https://index.docker.io/v1/"]
        } else {
            vec![host]
        }
        .into_iter()
    }

    /// Returns the auth for the host.
    ///
    /// Some hosts have fallback keys; the host that actually was used to retrieve the auth
    /// is returned so that if it was a fallback key the correct key can be used to
    /// retrieve auth information in subsequent operations.
    async fn auth(&self, host: &str) -> Result<Authentication> {
        for key in Self::auth_keys(host) {
            if let Some(auth) = self.auths.get(key) {
                match auth.decode(self, key).await {
                    Ok(auth) => return Ok(auth),
                    Err(err) => {
                        warn!("failed decoding auth for host {key:?}: {err:?}");
                        continue;
                    }
                }
            }
        }

        Ok(Authentication::None)
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DockerAuth {
    /// The credentials are stored in plain text, not in a helper.
    Plain {
        /// Base64 encoded authentication credentials in the form of `username:password`.
        auth: String,
    },

    /// The credentials are stored in a helper.
    /// Use the host with the top level [`DockerConfig`] to determine which helper to use.
    Helper {},
}

impl DockerAuth {
    async fn decode(&self, config: &DockerConfig, host: &str) -> Result<Authentication> {
        match self {
            DockerAuth::Plain { auth } => Self::decode_plain(auth),
            DockerAuth::Helper {} => Self::decode_helper(config, host).await,
        }
    }

    fn decode_plain(auth: &str) -> Result<Authentication> {
        let auth = base64::engine::general_purpose::STANDARD
            .decode(auth)
            .context("decode base64 auth key")?;
        let auth = String::from_utf8(auth).context("parse auth key as utf-8")?;
        let (username, password) = auth
            .split_once(':')
            .ok_or_eyre("invalid auth key format, expected username:password")?;
        Ok(Authentication::basic(username, password))
    }

    async fn decode_helper(config: &DockerConfig, host: &str) -> Result<Authentication> {
        let helper = config
            .cred_helpers
            .get(host)
            .or(config.creds_store.as_ref())
            .ok_or_eyre("no helper found for host")?;

        let binary = format!("docker-credential-{helper}");
        let mut exec = tokio::process::Command::new(&binary)
            .arg("get")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawn docker credential helper")
            .with_section(|| binary.clone().header("Helper binary:"))?;

        if let Some(mut stdin) = exec.stdin.take() {
            stdin
                .write_all(host.as_bytes())
                .await
                .context("write request to helper")?;
            drop(stdin);
        }

        let output = exec.wait_with_output().await.context("run helper")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            return Err(eyre!("auth helper failed with status: {}", output.status))
                .with_section(|| binary.clone().header("Helper binary:"))
                .with_section(|| host.to_string().header("Host:"))
                .with_section(|| output.status.to_string().header("Command status code:"))
                .with_section(|| stderr.header("Stderr:"))
                .with_section(|| stdout.header("Stdout:"));
        }

        let credential = serde_json::from_slice::<DockerCredential>(&output.stdout)
            .context("decode helper output")
            .with_section(|| binary.header("Helper binary:"))?;
        Ok(Authentication::basic(
            credential.username,
            credential.secret,
        ))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerCredential {
    username: String,
    secret: String,
}

/// Each instance is a unique view of a local Docker daemon for a specific [`Reference`].
/// Similar to [`crate::registry::Registry`], but interacts with a local Docker daemon.
#[derive(Debug)]
#[allow(unused)]
pub struct Daemon {
    /// The client used to interact with the docker daemon.
    #[debug(skip)]
    docker: Docker,

    /// The image ID in the daemon being referenced.
    image: String,

    /// The file on disk representing the exported container.
    exported: TempFile,

    /// Layer filters.
    /// Layers that match any filter are excluded from the set of layers processed.
    layer_filters: Filters,

    /// File filters.
    /// Files that match any filter are excluded from the set of files processed.
    file_filters: Filters,
}

#[bon::bon]
impl Daemon {
    /// Create a new daemon for a specific reference.
    #[builder]
    pub async fn new(
        /// Filters for layers.
        /// Layers that match any filter are excluded from the set of layers processed.
        #[builder(into)]
        layer_filters: Option<Filters>,

        /// Filters for files.
        /// Files that match any filter are excluded from the set of files processed.
        #[builder(into)]
        file_filters: Option<Filters>,

        /// The reference for the image the user provided.
        #[builder(into)]
        reference: String,
    ) -> Result<Self> {
        let docker = Docker::connect_with_local_defaults().context("connect to docker daemon")?;
        let image = find_image(&docker, &reference)
            .await
            .context("find image")?;

        let stream = docker.export_image(&image);
        let exported = cfs::collect_tmp(stream)
            .await
            .context("collect exported image")?;

        Ok(Self {
            docker,
            image,
            exported,
            layer_filters: layer_filters.unwrap_or_default(),
            file_filters: file_filters.unwrap_or_default(),
        })
    }
}

impl Daemon {
    /// Report the digest for the image.
    #[tracing::instrument]
    pub async fn digest(&self) -> Result<Digest> {
        Digest::from_sha256(&self.image).context("parse image ID as sha256")
    }

    /// Enumerate layers for a container image from the local Docker daemon.
    /// Layers are returned in order from the base image to the application.
    #[tracing::instrument]
    pub async fn layers(&self) -> Result<Vec<Layer>> {
        todo!()
    }

    /// Pull the bytes of a layer from the daemon in a stream.
    #[tracing::instrument]
    pub async fn pull_layer(
        &self,
        layer: &Layer,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        todo!()
    }

    /// Enumerate files in a layer.
    #[tracing::instrument]
    pub async fn list_files(&self, layer: &Layer) -> Result<Vec<String>> {
        todo!()
    }

    /// Apply a layer to a location on disk.
    ///
    /// The intention of this method is that when it is run for each layer in an image in order it is equivalent
    /// to the functionality you'd get by running `docker pull`, `docker save`, and then recursively extracting the
    /// layers to the same directory.
    #[tracing::instrument]
    pub async fn apply_layer(&self, layer: &Layer, output: &Path) -> Result<()> {
        todo!()
    }

    /// Normalize an OCI layer into a plain tarball layer.
    #[tracing::instrument]
    pub async fn layer_plain_tarball(&self, layer: &Layer) -> Result<Option<TempFile>> {
        todo!()
    }
}

impl Source for Daemon {
    async fn digest(&self) -> Result<Digest> {
        self.digest().await
    }

    async fn name(&self) -> Result<String> {
        Ok(self.image.clone())
    }

    async fn layers(&self) -> Result<Vec<Layer>> {
        self.layers().await
    }

    async fn pull_layer(
        &self,
        layer: &Layer,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        self.pull_layer(layer).await
    }

    async fn list_files(&self, layer: &Layer) -> Result<Vec<String>> {
        self.list_files(layer).await
    }

    async fn apply_layer(&self, layer: &Layer, output: &Path) -> Result<()> {
        self.apply_layer(layer, output).await
    }

    async fn layer_plain_tarball(&self, layer: &Layer) -> Result<Option<TempFile>> {
        self.layer_plain_tarball(layer).await
    }
}

/// Docker tarball manifest file format
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct DockerManifest {
    /// Path to the image configuration file
    pub config: String,

    /// Repository tags associated with the image
    #[serde(default)]
    pub repo_tags: Vec<String>,

    /// Layer tar filenames in order (from base to application)
    pub layers: Vec<String>,
}

/// An implementation of [`Source`] that reads from a local docker tarball.
///
/// Docker tarballs are created via the `docker save` command and have a specific
/// structure with a manifest.json file and layer tarballs.
#[derive(Debug)]
pub struct Tarball {
    /// Path to the Docker tarball file
    path: PathBuf,

    /// The parsed manifest from the tarball
    manifest: DockerManifest,

    /// Digest computed from the image configuration
    image_digest: Digest,

    /// Name inferred from the tarball
    name: String,

    /// Cached list of layers
    layers: Vec<Layer>,

    /// Layer filters.
    /// Layers that match any filter are excluded from the set of layers processed.
    layer_filters: Filters,

    /// File filters.
    /// Files that match any filter are excluded from the set of files processed.
    file_filters: Filters,
}

#[bon::bon]
impl Tarball {
    /// Create a new tarball source from a path to a Docker tarball.
    #[builder]
    pub async fn new(
        /// Path to the Docker tarball file
        #[builder(into)]
        path: PathBuf,

        /// Filters for layers.
        /// Layers that match any filter are excluded from the set of layers processed.
        #[builder(into)]
        layer_filters: Option<Filters>,

        /// Filters for files.
        /// Files that match any filter are excluded from the set of files processed.
        #[builder(into)]
        file_filters: Option<Filters>,
    ) -> Result<Self> {
        if !path.exists() {
            return Err(eyre!("Docker tarball not found: {}", path.display()))
                .with_section(|| path.display().to_string().header("Path:"));
        }

        let manifest = read_docker_manifest(&path).await?;
        let image_digest = compute_image_digest(&path, &manifest).await?;
        let name = infer_name(&manifest, &image_digest);
        let layers = parse_layers(&path, &manifest).await?;

        Ok(Self {
            path,
            manifest,
            image_digest,
            name,
            layers,
            layer_filters: layer_filters.unwrap_or_default(),
            file_filters: file_filters.unwrap_or_default(),
        })
    }
}

impl Tarball {
    /// Report the digest for the image.
    #[tracing::instrument]
    pub async fn digest(&self) -> Result<Digest> {
        Ok(self.image_digest.clone())
    }

    /// Report the name of the image.
    #[tracing::instrument]
    pub async fn name(&self) -> Result<String> {
        Ok(self.name.clone())
    }

    /// Enumerate layers for a container image.
    /// Layers are returned in order from the base image to the application.
    #[tracing::instrument]
    pub async fn layers(&self) -> Result<Vec<Layer>> {
        // Filter layers if filters are set
        let filtered = self
            .layers
            .iter()
            .filter(|&layer| self.layer_filters.matches(layer))
            .cloned()
            .collect();

        Ok(filtered)
    }

    /// Pull the bytes of a layer from the tarball in a stream.
    #[tracing::instrument]
    pub async fn pull_layer(
        &self,
        layer: &Layer,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        // Find the layer index within our manifests
        let layer_index = self.find_layer_index(layer)?;
        let layer_path = Path::new(&self.manifest.layers[layer_index]);

        // Create a temporary reader for the tarball
        let reader = create_layer_reader(&self.path, layer_path).await?;

        // Map std::io::Error to color_eyre::Report
        let stream = reader.map(|result| result.map_err(|e| eyre!("Error reading layer: {e}")));

        // Return a stream of bytes
        Ok(Box::pin(stream))
    }

    /// Enumerate files in a layer.
    #[tracing::instrument]
    pub async fn list_files(&self, layer: &Layer) -> Result<Vec<String>> {
        let stream = self.pull_layer_internal(layer).await?;

        match &layer.media_type {
            LayerMediaType::Oci(flags) => {
                match flags.as_slice() {
                    // No flags; this means the layer is uncompressed
                    [] => enumerate_tarball(stream).await,

                    // The layer is compressed with gzip
                    [LayerMediaTypeFlag::Gzip] => {
                        let stream = transform::gzip(stream);
                        enumerate_tarball(stream).await
                    }

                    // The layer is compressed with zstd
                    [LayerMediaTypeFlag::Zstd] => {
                        let stream = transform::zstd(stream);
                        enumerate_tarball(stream).await
                    }

                    // More complex transformation sequence
                    _ => {
                        let stream = transform::sequence(stream, flags);
                        enumerate_tarball(stream).await
                    }
                }
            }
        }
    }

    /// Apply a layer to a location on disk.
    ///
    /// The intention of this method is that when it is run for each layer in an image in order it is equivalent
    /// to the functionality you'd get by running `docker pull`, `docker save`, and then recursively extracting the
    /// layers to the same directory.
    #[tracing::instrument]
    pub async fn apply_layer(&self, layer: &Layer, output: &Path) -> Result<()> {
        let stream = self.pull_layer_internal(layer).await?;

        match &layer.media_type {
            LayerMediaType::Oci(flags) => {
                // Skip foreign layers
                if flags.contains(&LayerMediaTypeFlag::Foreign) {
                    warn!("skip: foreign layer");
                    return Ok(());
                }

                match flags.as_slice() {
                    // No flags; this means the layer is uncompressed
                    [] => apply_tarball(&self.file_filters, stream, output).await,

                    // The layer is compressed with gzip
                    [LayerMediaTypeFlag::Gzip] => {
                        let stream = transform::gzip(stream);
                        apply_tarball(&self.file_filters, stream, output).await
                    }

                    // The layer is compressed with zstd
                    [LayerMediaTypeFlag::Zstd] => {
                        let stream = transform::zstd(stream);
                        apply_tarball(&self.file_filters, stream, output).await
                    }

                    // More complex transformation sequence
                    _ => {
                        let stream = transform::sequence(stream, flags);
                        apply_tarball(&self.file_filters, stream, output).await
                    }
                }
            }
        }
    }

    /// Normalize an OCI layer into a plain tarball layer.
    #[tracing::instrument]
    pub async fn layer_plain_tarball(&self, layer: &Layer) -> Result<Option<TempFile>> {
        let stream = self.pull_layer_internal(layer).await?;

        match &layer.media_type {
            LayerMediaType::Oci(flags) => {
                // Skip foreign layers
                if flags.contains(&LayerMediaTypeFlag::Foreign) {
                    warn!("skip: foreign layer");
                    return Ok(None);
                }

                Ok(Some(match flags.as_slice() {
                    // No flags; this means the layer is already uncompressed
                    [] => collect_tmp(stream).await?,

                    // The layer is compressed with gzip
                    [LayerMediaTypeFlag::Gzip] => transform::gzip(stream).pipe(collect_tmp).await?,

                    // The layer is compressed with zstd
                    [LayerMediaTypeFlag::Zstd] => transform::zstd(stream).pipe(collect_tmp).await?,

                    // More complex transformation sequence
                    _ => transform::sequence(stream, flags).pipe(collect_tmp).await?,
                }))
            }
        }
    }

    /// Find the layer index in the manifest that corresponds to this layer
    fn find_layer_index(&self, layer: &Layer) -> Result<usize> {
        self.layers
            .iter()
            .position(|l| l.digest == layer.digest)
            .ok_or_else(|| eyre!("Layer not found in tarball: {}", layer.digest))
    }

    /// Internal helper for pulling layer data
    async fn pull_layer_internal(&self, layer: &Layer) -> Result<impl Stream<Item = Chunk>> {
        // Find the layer index within our manifests
        let layer_index = self.find_layer_index(layer)?;
        let layer_path = Path::new(&self.manifest.layers[layer_index]);

        // Create a reader for the layer
        create_layer_reader(&self.path, layer_path).await
    }
}

/// Read manifest from a Docker tarball
async fn read_docker_manifest(tarball_path: &Path) -> Result<DockerManifest> {
    debug!(
        "Reading manifest from Docker tarball: {}",
        tarball_path.display()
    );

    let file = tokio::fs::File::open(tarball_path).await.context(format!(
        "opening docker tarball: {}",
        tarball_path.display()
    ))?;

    let mut archive = Archive::new(file);
    let mut manifest_entry = None;

    let mut entries = archive.entries().context(format!(
        "reading entries from tarball: {}",
        tarball_path.display()
    ))?;

    while let Some(entry_result) = entries.next().await {
        let entry = entry_result.context("reading tarball entry")?;
        let path = entry.path().context("reading entry path")?;
        let path = path.to_string_lossy();

        if path == "manifest.json" || path.ends_with("/manifest.json") {
            debug!("Found manifest at path: {path}");
            manifest_entry = Some(entry);
            break;
        }
    }

    let mut manifest_entry = manifest_entry.ok_or_else(|| {
        eyre!("manifest.json not found in Docker tarball")
            .with_section(|| tarball_path.display().to_string().header("Tarball path:"))
    })?;

    let mut manifest_contents = String::new();
    manifest_entry
        .read_to_string(&mut manifest_contents)
        .await
        .context("reading manifest.json contents")?;

    debug!("Parsing manifest.json content");

    // Try first as an array (standard format), then as a single object
    let array_result: Result<Vec<DockerManifest>, _> = serde_json::from_str(&manifest_contents);

    match array_result {
        Ok(mut manifests) => {
            if manifests.is_empty() {
                return Err(eyre!("Docker tarball contains empty manifest.json array"))
                    .with_section(|| tarball_path.display().to_string().header("Tarball path:"));
            }

            debug!(
                "Successfully parsed manifest.json as array with {} entries",
                manifests.len()
            );
            Ok(manifests.remove(0))
        }

        Err(_) => {
            debug!("Manifest is not an array, trying to parse as single object");
            let single_result: Result<DockerManifest, _> = serde_json::from_str(&manifest_contents);

            match single_result {
                Ok(manifest) => {
                    debug!("Successfully parsed manifest.json as single object");
                    Ok(manifest)
                }
                Err(err) => {
                    // Both parsing attempts failed, return the original error
                    Err(eyre!("Failed to parse manifest.json: {err}"))
                        .with_section(|| tarball_path.display().to_string().header("Tarball path:"))
                        .with_section(|| manifest_contents.to_string().header("Manifest content:"))
                }
            }
        }
    }
}

/// Compute the image digest from the config file
async fn compute_image_digest(tarball_path: &Path, manifest: &DockerManifest) -> Result<Digest> {
    let file = tokio::fs::File::open(tarball_path)
        .await
        .context("opening docker tarball")?;

    let mut archive = Archive::new(file);
    let mut config_entry = None;

    let config_path = &manifest.config;
    let mut entries = archive.entries().context("reading tarball entries")?;
    while let Some(entry_result) = entries.next().await {
        let entry = entry_result.context("read tarball entry")?;
        let path = entry.path().context("read entry path")?;

        if path.to_string_lossy() == config_path.as_str() {
            config_entry = Some(entry);
            break;
        }
    }

    let mut config_entry = config_entry
        .ok_or_else(|| eyre!("config file not found in Docker tarball: {}", config_path))?;

    let mut config_data = Vec::new();
    config_entry
        .read_to_end(&mut config_data)
        .await
        .context("reading config file contents")?;

    let mut hasher = sha2::Sha256::new();
    use sha2::Digest as Sha256Digest;
    hasher.update(&config_data);
    let hash = hasher.finalize();

    Ok(crate::Digest {
        algorithm: crate::Digest::SHA256.to_string(),
        hash: hash.to_vec(),
    })
}

/// Infer the image name from the manifest
fn infer_name(manifest: &DockerManifest, digest: &Digest) -> String {
    // Try to get name from repo tags
    if let Some(tag) = manifest.repo_tags.first() {
        // Return the tag without the version part
        if let Some(colon_pos) = tag.rfind(':') {
            return tag[..colon_pos].to_string();
        }
        return tag.clone();
    }

    // Fallback to using digest
    format!("image@{digest}")
}

/// Parse the layers from the manifest
async fn parse_layers(tarball_path: &Path, manifest: &DockerManifest) -> Result<Vec<Layer>> {
    let mut layers = Vec::new();

    for layer_file in &manifest.layers {
        let layer_size = get_layer_size(tarball_path, layer_file).await?;
        let digest = extract_digest_from_layer_path(layer_file)?;
        let media_type = detect_layer_media_type(tarball_path, layer_file).await?;

        layers.push(Layer {
            digest,
            size: layer_size as i64,
            media_type,
        });
    }

    Ok(layers)
}

/// Detect the media type of a layer by examining its content
async fn detect_layer_media_type(tarball_path: &Path, layer_file: &str) -> Result<LayerMediaType> {
    // Most Docker tarballs use gzip compression by default
    let default_type = LayerMediaType::Oci(vec![LayerMediaTypeFlag::Gzip]);

    let file = match tokio::fs::File::open(tarball_path).await {
        Ok(f) => f,
        Err(_) => return Ok(default_type), // Return default on error
    };

    let mut archive = Archive::new(file);
    let mut layer_entry = None;

    let mut entries = match archive.entries() {
        Ok(e) => e,
        Err(_) => return Ok(default_type),
    };

    while let Some(entry_result) = entries.next().await {
        match entry_result {
            Ok(entry) => {
                if let Ok(path) = entry.path() {
                    if path.to_string_lossy() == layer_file {
                        layer_entry = Some(entry);
                        break;
                    }
                }
            }
            Err(_) => continue,
        }
    }

    let mut layer_entry = match layer_entry {
        Some(entry) => entry,
        None => return Ok(default_type),
    };

    // Read the first few bytes to determine compression format
    let mut buffer = [0u8; 8];
    match layer_entry.read_exact(&mut buffer).await {
        Ok(_) => {
            // Check for gzip magic bytes (1F 8B)
            if buffer[0] == 0x1F && buffer[1] == 0x8B {
                return Ok(LayerMediaType::Oci(vec![LayerMediaTypeFlag::Gzip]));
            }

            // Check for zstd magic bytes (28 B5 2F FD)
            if buffer[0] == 0x28 && buffer[1] == 0xB5 && buffer[2] == 0x2F && buffer[3] == 0xFD {
                return Ok(LayerMediaType::Oci(vec![LayerMediaTypeFlag::Zstd]));
            }

            // If no compression detected, it's a raw tarball
            Ok(LayerMediaType::Oci(vec![]))
        }
        Err(_) => Ok(default_type),
    }
}

/// Extract the digest from a layer path in the Docker tarball
fn extract_digest_from_layer_path(layer_path: &str) -> Result<Digest> {
    // Docker layer paths can be in different formats:
    // 1. "<hash>/layer.tar" (most common in docker save output)
    // 2. "layers/<hash>/layer.tar" (also seen in some docker save outputs)
    // 3. "<hash>.tar" (rare but possible)

    // First, try to extract hash from the parent directory name
    if let Some(parent) = Path::new(layer_path).parent() {
        if let Some(filename) = parent.file_name() {
            if let Some(hash_str) = filename.to_str() {
                // Validate that it looks like a SHA256 hash (64 hex characters)
                if hash_str.len() == 64 && hash_str.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Ok(Digest {
                        algorithm: Digest::SHA256.to_string(),
                        hash: hex::decode(hash_str)
                            .context(format!("Invalid hex in digest: {hash_str}"))?,
                    });
                }
            }
        }
    }

    // Alternative: try to extract from the filename itself (for <hash>.tar format)
    if let Some(filename) = Path::new(layer_path).file_stem() {
        if let Some(hash_str) = filename.to_str() {
            // Validate that it looks like a SHA256 hash (64 hex characters)
            if hash_str.len() == 64 && hash_str.chars().all(|c| c.is_ascii_hexdigit()) {
                return Ok(Digest {
                    algorithm: Digest::SHA256.to_string(),
                    hash: hex::decode(hash_str)
                        .context(format!("Invalid hex in digest: {hash_str}"))?,
                });
            }
        }
    }

    // If we can't determine a hash from the path, generate one from the content
    // This is a fallback that should rarely be needed with standard Docker tarballs
    Err(eyre!("Cannot extract hash from layer path: {layer_path}\nThis doesn't appear to be a standard Docker layer path format."))
}

/// Get the size of a layer tarball within the docker tarball
async fn get_layer_size(tarball_path: &Path, layer_file: &str) -> Result<u64> {
    let file = tokio::fs::File::open(tarball_path)
        .await
        .context("opening docker tarball")?;

    let mut archive = Archive::new(file);

    // Find the layer file entry
    let mut entries = archive.entries().context("reading tarball entries")?;
    while let Some(entry_result) = entries.next().await {
        let entry = entry_result.context("reading tarball entry")?;
        let path = entry.path().context("reading entry path")?;

        if path.to_string_lossy() == layer_file {
            return Ok(entry.header().size().unwrap_or(0));
        }
    }

    Err(eyre!("Layer file not found in tarball: {layer_file}"))
}

/// Create a reader for a specific layer in the docker tarball
///
/// This is a bit inefficient as it needs to scan the tarball for each layer access,
/// but it's more memory-efficient than loading the whole tarball at once.
async fn create_layer_reader(
    tarball_path: &Path,
    layer_path: &Path,
) -> Result<impl Stream<Item = Chunk>> {
    debug!(
        "Opening layer {} from Docker tarball {}",
        layer_path.display(),
        tarball_path.display()
    );

    if !tarball_path.exists() {
        return Err(eyre!(
            "Docker tarball not found: {}",
            tarball_path.display()
        ))
        .with_section(|| tarball_path.display().to_string().header("Tarball path:"));
    }

    let file = tokio::fs::File::open(tarball_path).await.context(format!(
        "opening docker tarball: {}",
        tarball_path.display()
    ))?;

    let mut archive = Archive::new(file);
    let mut layer_entry = None;

    let layer_path = layer_path.to_string_lossy();
    debug!("Searching for layer: {layer_path}");

    let mut entries = archive.entries().context(format!(
        "reading entries from tarball: {}",
        tarball_path.display()
    ))?;

    while let Some(entry_result) = entries.next().await {
        let entry = entry_result.context("reading tarball entry")?;
        let path = entry.path().context("reading entry path")?;
        let path = path.to_string_lossy();

        if path == layer_path {
            debug!("Found exact match for layer: {layer_path}");
            layer_entry = Some(entry);
            break;
        }

        // Sometimes Docker tarballs can have inconsistent path separators or extra components
        // Try normalizing paths for comparison
        let normalized_entry = Path::new(&*path)
            .file_name()
            .map(|f| f.to_string_lossy());

        let normalized_layer = Path::new(&*layer_path)
            .file_name()
            .map(|f| f.to_string_lossy());

        if let (Some(ne), Some(nl)) = (normalized_entry, normalized_layer) {
            if ne == nl {
                debug!("Found normalized match for layer: {layer_path} as {path}");
                layer_entry = Some(entry);
                break;
            }
        }
    }

    if layer_entry.is_none() {
        debug!("Layer not found with exact match, trying flexible search: {layer_path}");

        let file = tokio::fs::File::open(tarball_path).await.context(format!(
            "reopening docker tarball: {}",
            tarball_path.display()
        ))?;

        let mut archive = Archive::new(file);
        let mut entries = archive.entries().context(format!(
            "reading entries from tarball (second pass): {}",
            tarball_path.display()
        ))?;

        let target_filename = Path::new(&*layer_path)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "layer.tar".to_string());

        while let Some(entry_result) = entries.next().await {
            let entry = entry_result.context("reading tarball entry")?;
            let path = entry.path().context("reading entry path")?;

            if path.to_string_lossy().ends_with(&target_filename) {
                debug!(
                    "Found partial match for layer: {layer_path} as {}",
                    path.to_string_lossy()
                );
                layer_entry = Some(entry);
                break;
            }
        }
    }

    let layer_entry = layer_entry.ok_or_else(|| {
        eyre!(
            "Layer {} not found in Docker tarball {}",
            layer_path,
            tarball_path.display()
        )
        .with_section(|| layer_path.to_string().header("Layer path:"))
        .with_section(|| tarball_path.display().to_string().header("Tarball path:"))
    })?;

    debug!("Successfully found and opened layer: {layer_path}");
    Ok(ReaderStream::new(layer_entry))
}

impl Source for Tarball {
    async fn digest(&self) -> Result<Digest> {
        self.digest().await
    }

    async fn name(&self) -> Result<String> {
        self.name().await
    }

    async fn layers(&self) -> Result<Vec<Layer>> {
        self.layers().await
    }

    async fn pull_layer(
        &self,
        layer: &Layer,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        self.pull_layer(layer).await
    }

    async fn list_files(&self, layer: &Layer) -> Result<Vec<String>> {
        self.list_files(layer).await
    }

    async fn apply_layer(&self, layer: &Layer, output: &Path) -> Result<()> {
        self.apply_layer(layer, output).await
    }

    async fn layer_plain_tarball(&self, layer: &Layer) -> Result<Option<TempFile>> {
        self.layer_plain_tarball(layer).await
    }
}

/// Find the ID of the image for the specified reference in the Docker daemon, if it exists.
/// If it doesn't exist, this function returns an error.
async fn find_image(docker: &Docker, reference: &str) -> Result<String> {
    let opts = bollard::image::ListImagesOptions::<String> {
        all: true,
        ..Default::default()
    };

    let images = docker
        .list_images(Some(opts))
        .await
        .context("list images")?;

    // Images in the docker daemon don't use the fully qualified reference,
    // they look like this:
    // ```
    // repo_tags: [
    //     "changeset_example:latest",
    // ],
    // repo_digests: [
    //     "changeset_example@sha256:1af7aa8d7fe18420f10b46a78c23c5c9cb01817d30a03a12c33e8a26555f7b4f",
    // ],
    // repo_tags: [
    //     "fossaeng/changeset_example:latest",
    // ],
    // repo_digests: [
    //     "fossaeng/changeset_example@sha256:495f92a2c50d0b1550b232213c19bd4b5121a2268f95f0b7be6bb1c7dd51c4ce",
    // ],
    // ```
    // As such, we just use the string the user provided;
    // if it matches any tag or digest it's good to go.

    // Collect the images
    let id_by_tag_or_digest = images
        .iter()
        .flat_map(|i| {
            i.repo_tags
                .iter()
                .map(|t| t.as_str())
                .chain(i.repo_digests.iter().map(|d| d.as_str()))
                .zip(std::iter::repeat(i.id.as_str()))
        })
        .collect::<HashMap<_, _>>();

    if let Some(image) = id_by_tag_or_digest.get(reference) {
        return Ok(image.to_string());
    }

    let listings = id_by_tag_or_digest.keys().collect::<Vec<_>>();
    Err(eyre!("image not found: {reference}"))
        .with_note(|| format!("{listings:#?}").header("Images:"))
}
