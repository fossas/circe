use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    pin::Pin,
    process::Stdio,
    str::FromStr,
};

use crate::{
    cfs::{self, apply_tarball, collect_tmp, enumerate_tarball},
    ext::PriorityFind,
    homedir,
    transform::{self, Chunk},
    Authentication, Digest, Filter, FilterMatch, Filters, Layer, LayerMediaType,
    LayerMediaTypeFlag, Reference, Source, Version,
};
use async_tempfile::TempFile;
use base64::Engine;
use bollard::image::RemoveImageOptions;
use bollard::models::ImageInspect as BollardImageInspect;
use bollard::service::ImageInspect;
use bollard::Docker;
use bollard::{container::RemoveContainerOptions, secret::ImageSummary};
use bytes::Bytes;
use color_eyre::{
    eyre::{bail, ensure, eyre, Context, OptionExt, Result},
    Section, SectionExt,
};
use derive_more::Debug;
use futures_lite::{Stream, StreamExt};
use serde::Deserialize;
use tap::{Pipe, TapFallible};
use tokio::io::{AsyncRead, AsyncWriteExt, BufWriter};
use tokio_tar::{Archive, Entry};
use tokio_util::io::StreamReader;
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
    fn auth_keys<'a>(host: &'a str) -> impl Iterator<Item = &'a str> {
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
            .decode(&auth)
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
pub struct Daemon {
    /// The client used to interact with the docker daemon.
    #[debug(skip)]
    docker: Docker,

    /// The image ID in the daemon being referenced.
    image: String,

    /// The file on disk representing the exported container.
    exported: TempFile,

    /// Layer filters.
    /// Layers that match any filter are included in the set of layers processed by this daemon.
    layer_filters: Filters,

    /// File filters.
    /// Files that match any filter are included in the set of files processed by this daemon.
    file_filters: Filters,
}

#[bon::bon]
impl Daemon {
    /// Create a new daemon for a specific reference.
    #[builder]
    pub async fn new(
        /// Filters for layers.
        /// Layers that match any filter are included in the set of layers processed by this daemon.
        #[builder(into)]
        layer_filters: Option<Filters>,

        /// Filters for files.
        /// Files that match any filter are included in the set of files processed by this daemon.
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
