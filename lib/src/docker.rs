use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    pin::Pin,
    process::Stdio,
};

use crate::{
    cio::{
        self, apply_tarball, collect_json, collect_tmp, enumerate_tarball, extract_file,
        extract_json, file_digest, peel_layer,
    },
    homedir,
    transform::Chunk,
    Authentication, Digest, FilterMatch, Filters, Layer, Reference, Source,
};
use async_tempfile::TempFile;
use base64::Engine;
use bollard::Docker;
use bytes::Bytes;
use color_eyre::{
    eyre::{eyre, Context, Error, OptionExt, Result},
    Section, SectionExt,
};
use derive_more::Debug;
use futures_lite::{Stream, StreamExt};
use serde::Deserialize;
use tap::{Pipe, TapFallible};
use tokio::{fs::File, io::AsyncWriteExt};
use tokio_tar::{Archive, Entry};
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
pub struct Daemon {
    /// The file on disk representing the exported container.
    ///
    /// This is referenced in [`Tarball`] by path; in order to keep tarball generic
    /// it doesn't actually take ownership of the tempfile handle itself.
    #[debug(skip)]
    _exported: TempFile,

    /// References the exported local tarball.
    tarball: Tarball,
}

#[bon::bon]
impl Daemon {
    /// Create a new daemon for a specific reference.
    #[builder]
    #[tracing::instrument(name = "Daemon::new")]
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
        crate::flag_disabled_daemon_docker()?;

        let docker = Docker::connect_with_local_defaults().context("connect to docker daemon")?;
        let image = find_image(&docker, &reference)
            .await
            .context("find image")?;

        let stream = docker.export_image(&image);
        let exported = cio::collect_tmp(stream)
            .await
            .context("collect exported image")?;

        debug!(exported = ?exported.file_path(), "exported temporary image");
        let tarball = Tarball::builder()
            .maybe_file_filters(file_filters)
            .maybe_layer_filters(layer_filters)
            .name(image)
            .path(exported.file_path())
            .build()
            .await
            .context("create tarball")?;

        debug!(tarball = ?tarball.path, "created tarball");
        Ok(Self {
            _exported: exported,
            tarball,
        })
    }
}

impl Source for Daemon {
    async fn digest(&self) -> Result<Digest> {
        self.tarball.digest().await
    }

    async fn name(&self) -> Result<String> {
        self.tarball.name().await
    }

    async fn layers(&self) -> Result<Vec<Layer>> {
        self.tarball.layers().await
    }

    async fn pull_layer(
        &self,
        layer: &Layer,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        self.tarball.pull_layer(layer).await
    }

    async fn list_files(&self, layer: &Layer) -> Result<Vec<String>> {
        self.tarball.list_files(layer).await
    }

    async fn apply_layer(&self, layer: &Layer, output: &Path) -> Result<()> {
        self.tarball.apply_layer(layer, output).await
    }

    async fn layer_plain_tarball(&self, layer: &Layer) -> Result<Option<TempFile>> {
        self.tarball.layer_plain_tarball(layer).await
    }
}

/// An implementation of [`Source`] that reads from a local docker tarball.
///
/// Docker tarballs are created via the `docker save` command.
/// The legacy Docker tarball format (indicated by `manifest.json`)
/// and the modern OCI tarball format (indicated by `index.json`)
/// are both presented in the tarball alongside one another;
/// Circe only interacts with the OCI format.
///
/// If the tarball is legacy format, extraction will fail.
#[derive(Debug)]
pub struct Tarball {
    /// Path to the Docker tarball file.
    path: PathBuf,

    /// The parsed manifest from the tarball.
    manifest: DockerManifest,

    /// Digest computed from the image configuration.
    digest: Digest,

    /// Name of the docker image.
    name: String,

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
        /// Name of the docker image.
        #[builder(into)]
        name: String,

        /// Path to the Docker tarball file.
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

        let digest = digest(&path).await.context("compute digest")?;
        let manifests = DockerManifest::peel(&path)
            .await
            .context("peel manifests")?;
        let manifest = manifests.first().cloned().ok_or_eyre("no manifest found")?;
        if manifests.len() > 1 {
            tracing::warn!(
                ?manifests,
                "multiple manifests found in tarball, using first one"
            );
        }

        Ok(Self {
            path,
            manifest,
            digest,
            name,
            layer_filters: layer_filters.unwrap_or_default(),
            file_filters: file_filters.unwrap_or_default(),
        })
    }
}

impl Tarball {
    async fn pull_layer_internal(&self, layer: &Layer) -> Result<impl Stream<Item = Chunk>> {
        let name = layer.digest.as_hex();
        extract_file(&self.path, move |path| path.ends_with(&name))
            .await
            .context("extract layer tarball")?
            .ok_or_eyre("layer not found")
    }
}

/// A Docker OCI manifest.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DockerManifest {
    /// The layers in the manifest.
    #[debug(skip)]
    layers: Vec<Layer>,
}

impl DockerManifest {
    /// Recursively peel the manifest from the tarball.
    ///
    /// OCI Docker images can have multiple layers of indices,
    /// for example the outer `index.json` might look like this:
    /// ```not_rust
    /// {
    ///   "schemaVersion": 2,
    ///   "mediaType": "application/vnd.oci.image.index.v1+json",
    ///   "manifests": [
    ///     {
    ///       "mediaType": "application/vnd.oci.image.index.v1+json",
    ///       "digest": "sha256:1af7aa8d7fe18420f10b46a78c23c5c9cb01817d30a03a12c33e8a26555f7b4f",
    ///       "size": 856,
    ///       "annotations": {
    ///         "containerd.io/distribution.source.docker.io": "fossaeng/changeset_example",
    ///         "io.containerd.image.name": "docker.io/library/changeset_example:latest",
    ///         "org.opencontainers.image.ref.name": "latest"
    ///       }
    ///     }
    ///   ]
    /// }
    /// ```
    ///
    /// This then points (via `digest`) to another index like this:
    /// ```not_rust
    /// {
    ///   "schemaVersion": 2,
    ///   "mediaType": "application/vnd.oci.image.index.v1+json",
    ///   "manifests": [
    ///     {
    ///       "mediaType": "application/vnd.oci.image.manifest.v1+json",
    ///       "digest": "sha256:2dbf67cffe2b7bce89eeee6a34ad3d800e9b3bba16a4fdd7c349d6c5d12ccebf",
    ///       "size": 1795,
    ///       "platform": {
    ///         "architecture": "arm64",
    ///         "os": "linux"
    ///       }
    ///     },
    ///     {
    ///       "mediaType": "application/vnd.oci.image.manifest.v1+json",
    ///       "digest": "sha256:26dcd7e5b09fd079c9906769060fbced838177b295f6019e1fd9f6eba56e6960",
    ///       "size": 566,
    ///       "annotations": {
    ///         "vnd.docker.reference.digest": "sha256:2dbf67cffe2b7bce89eeee6a34ad3d800e9b3bba16a4fdd7c349d6c5d12ccebf",
    ///         "vnd.docker.reference.type": "attestation-manifest"
    ///       },
    ///       "platform": {
    ///         "architecture": "unknown",
    ///         "os": "unknown"
    ///       }
    ///     }
    ///   ]
    /// }
    /// ```
    ///
    /// And only after following the first digest do we finally arrive at the manifest:
    /// ```not_rust
    /// {
    ///   "schemaVersion": 2,
    ///   "mediaType": "application/vnd.oci.image.manifest.v1+json",
    ///   "config": {
    ///     "mediaType": "application/vnd.oci.image.config.v1+json",
    ///     "digest": "sha256:e6ff862dc923df33a755473a441a77e31c20f78c05df64638ae18226ab5168e2",
    ///     "size": 2787
    ///   },
    ///   "layers": [
    ///     {
    ///       "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
    ///       "digest": "sha256:422ed46b1a92579f7c475c0c19fade6880a8d98f23a2b4ccfb77c265d4f72dfc",
    ///       "size": 2725148
    ///     },
    ///     ...
    ///   ]
    /// }
    /// ```
    ///
    /// So when we "peel" the manifest, this means that the program searches all the JSON files
    /// inside the tarball for valid manifests.
    // #[tracing::instrument]
    async fn peel(tarball: &Path) -> Result<Vec<DockerManifest>> {
        let archive = tokio::fs::File::open(tarball)
            .await
            .context("open docker tarball")?;

        let mut archive = Archive::new(archive);
        archive.entries().context("read entries")?.then(
            async |entry: Result<Entry<Archive<File>>, std::io::Error>| -> Result<Option<DockerManifest>> {
                let entry = entry.context("read tarball entry")?;
                let path = entry.path().context("read entry path")?.to_path_buf();
                info!(?path, "evaluate for manifest");

                // If there's a parse error, it just means
                // the file wasn't an OCI manifest file.
                let stream = ReaderStream::new(entry);
                match collect_json(stream).await {
                    Ok(manifest) => Ok(Some(manifest)),
                    Err(err) => {
                        debug!(?path, ?err, "error parsing manifest");
                        Ok(None)
                    },
                }
            },
        )
        .filter_map(|manifest| manifest.transpose())
        .try_collect::<_, Error, Vec<_>>()
        .await
        .context("search archive for manifests")
    }
}

impl Source for Tarball {
    async fn digest(&self) -> Result<Digest> {
        Ok(self.digest.clone())
    }

    async fn name(&self) -> Result<String> {
        Ok(self.name.clone())
    }

    async fn layers(&self) -> Result<Vec<Layer>> {
        self.manifest
            .layers
            .iter()
            .filter(|&layer| !self.layer_filters.matches(layer))
            .cloned()
            .collect::<Vec<_>>()
            .pipe(Ok)
    }

    async fn pull_layer(
        &self,
        layer: &Layer,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        let stream = self.pull_layer_internal(layer).await?;
        Ok(Box::pin(stream.map(|chunk| chunk.context("read chunk"))))
    }

    async fn list_files(&self, layer: &Layer) -> Result<Vec<String>> {
        let stream = self.pull_layer_internal(layer).await?;
        match peel_layer(layer, stream) {
            Some(stream) => enumerate_tarball(stream).await,
            None => Ok(vec![]),
        }
    }

    async fn apply_layer(&self, layer: &Layer, output: &Path) -> Result<()> {
        let stream = self.pull_layer_internal(layer).await?;
        match peel_layer(layer, stream) {
            Some(stream) => apply_tarball(&self.file_filters, stream, output).await,
            None => Ok(()),
        }
    }

    async fn layer_plain_tarball(&self, layer: &Layer) -> Result<Option<TempFile>> {
        let stream = self.pull_layer_internal(layer).await?;
        match peel_layer(layer, stream) {
            Some(stream) => collect_tmp(stream).await.map(Some),
            None => Ok(None),
        }
    }
}

/// Find the ID of the image for the specified reference in the Docker daemon, if it exists.
/// If it doesn't exist, this function returns an error.
#[tracing::instrument]
async fn find_image(docker: &Docker, reference: &str) -> Result<String> {
    let opts = bollard::image::ListImagesOptions::<String> {
        all: true,
        ..Default::default()
    };

    let images = docker
        .list_images(Some(opts))
        .await
        .context("list images")?;
    debug!(?images, "listed images");

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
        debug!(?image, "found image");
        return Ok(image.to_string());
    }

    let listings = id_by_tag_or_digest.keys().collect::<Vec<_>>();
    Err(eyre!("image not found: {reference}"))
        .with_note(|| format!("{listings:#?}").header("Images:"))
}

/// Extract the digest for the docker image.
/// Tries to use the first digest in `index.json` as the digest;
/// if this fails it just computes a digest from the tarball itself.
async fn digest(tarball: &Path) -> Result<Digest> {
    #[derive(Debug, Deserialize)]
    struct Index {
        manifests: Vec<Manifest>,
    }

    #[derive(Debug, Deserialize)]
    struct Manifest {
        digest: Digest,
    }

    let is_index = |path: &Path| path.ends_with("index.json");
    if let Ok(Some(index)) = extract_json::<Index>(tarball, is_index).await {
        if let Some(manifest) = index.manifests.first() {
            return Ok(manifest.digest.clone());
        }
    }

    file_digest(tarball).await
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::{digest, Layer, LayerMediaType};

    #[test]
    fn parse_docker_manifest_nignx() {
        let content = include_str!("./testdata/nginx_manifest.json");

        let expected = DockerManifest {
            layers: vec![
                Layer {
                    digest: digest!(
                        "5f1ee22ffb5e68686db3dcb6584eb1c73b5570615b0f14fabb070b96117e351d"
                    ),
                    size: 77844480,
                    media_type: LayerMediaType::default(),
                },
                Layer {
                    digest: digest!(
                        "c68632c455ae0c46d1380033bae6d30014853fa3f600f4e14efc440be1bc9580"
                    ),
                    size: 118268416,
                    media_type: LayerMediaType::default(),
                },
                Layer {
                    digest: digest!(
                        "cabea05c000e49f0814b2611cbc66c2787f609d8a27fc7b9e97b5dab5d8502da"
                    ),
                    size: 3584,
                    media_type: LayerMediaType::default(),
                },
                Layer {
                    digest: digest!(
                        "791f0a07985c2814a899cb0458802be06ba124a364f7e5a9413a1f08fdbf5b5c"
                    ),
                    size: 4608,
                    media_type: LayerMediaType::default(),
                },
                Layer {
                    digest: digest!(
                        "f6d5815f290ee912fd4a768d97b46af39523dff584d786f5c0f7e9bdb7fad537"
                    ),
                    size: 2560,
                    media_type: LayerMediaType::default(),
                },
                Layer {
                    digest: digest!(
                        "7d22e2347c1217a89bd3c79ca9adb4652c1e9b61427fffc0ab92227aacd19a38"
                    ),
                    size: 5120,
                    media_type: LayerMediaType::default(),
                },
                Layer {
                    digest: digest!(
                        "55e9644f21c38d7707b4a432aacc7817c5414b68ac7a750e704c2f7100ebc15c"
                    ),
                    size: 7168,
                    media_type: LayerMediaType::default(),
                },
            ],
        };

        let manifest = serde_json::from_str(content).expect("parse manifest");
        pretty_assertions::assert_eq!(expected, manifest);
    }
}
