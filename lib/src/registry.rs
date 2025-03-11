//! Interacts with remote OCI registries.

use std::{
    path::{Path, PathBuf},
    pin::Pin,
    str::FromStr,
};

use async_tempfile::TempFile;
use bytes::Bytes;
use color_eyre::eyre::{Context, Result};
use derive_more::Debug;
use futures_lite::{Stream, StreamExt};
use oci_client::{
    client::ClientConfig,
    manifest::{ImageIndexEntry, OciDescriptor},
    secrets::RegistryAuth,
    Client, Reference as OciReference, RegistryOperation,
};
use tap::Pipe;
use tracing::{debug, warn};

use crate::{
    cfs::{apply_tarball, collect_tmp, enumerate_tarball},
    ext::PriorityFind,
    transform::{self, Chunk},
    Authentication, Digest, Filter, FilterMatch, Filters, Layer, LayerMediaType,
    LayerMediaTypeFlag, Platform, Reference, Source, Version,
};

/// Each instance is a unique view of remote registry for a specific [`Platform`] and [`Reference`].
/// The intention here is to better support chained methods like "pull list of layers" and then "apply each layer to disk".
// Note: internal fields aren't public because we don't want the caller to be able to mutate the internal state between method calls.
#[derive(Debug, Clone)]
pub struct Registry {
    /// The OCI reference, used by the underlying client.
    reference: OciReference,

    /// The original reference used to construct the registry.
    pub original: Reference,

    /// Authentication information for the registry.
    auth: RegistryAuth,

    /// Layer filters.
    /// Layers that match any filter are included in the set of layers processed by this registry.
    layer_filters: Filters,

    /// File filters.
    /// Files that match any filter are included in the set of files processed by this registry.
    file_filters: Filters,

    /// The client used to interact with the registry.
    #[debug(skip)]
    client: Client,
}

#[bon::bon]
impl Registry {
    /// Create a new registry for a specific platform and reference.
    #[builder]
    pub async fn new(
        /// Authentication information for the registry.
        auth: Option<Authentication>,

        /// The platform to use for the registry.
        platform: Option<Platform>,

        /// Filters for layers.
        /// Layers that match any filter are included in the set of layers processed by this registry.
        layer_filters: Option<Filters>,

        /// Filters for files.
        /// Files that match any filter are included in the set of files processed by this registry.
        file_filters: Option<Filters>,

        /// The reference to use for the registry.
        reference: Reference,
    ) -> Result<Self> {
        let client = client(platform.clone());
        let original = reference.clone();
        let reference = OciReference::from(&reference);
        let auth = auth
            .map(RegistryAuth::from)
            .unwrap_or(RegistryAuth::Anonymous);

        client
            .auth(&reference, &auth, RegistryOperation::Pull)
            .await
            .context("authenticate to registry")?;

        Ok(Self {
            auth,
            client,
            reference,
            original,
            layer_filters: layer_filters.unwrap_or_default(),
            file_filters: file_filters.unwrap_or_default(),
        })
    }
}

impl Registry {
    /// Enumerate layers for a container reference in the remote registry.
    /// Layers are returned in order from the base image to the application.
    #[tracing::instrument]
    pub async fn layers(&self) -> Result<Vec<Layer>> {
        let (manifest, _) = self
            .client
            .pull_image_manifest(&self.reference, &self.auth)
            .await
            .context("pull image manifest")?;
        manifest
            .layers
            .into_iter()
            .filter(|layer| self.layer_filters.matches(layer))
            .map(Layer::try_from)
            .collect()
    }

    /// Report the digest for the image.
    #[tracing::instrument]
    pub async fn digest(&self) -> Result<Digest> {
        let (_, digest) = self
            .client
            .pull_image_manifest(&self.reference, &self.auth)
            .await
            .context("pull image manifest")?;
        Digest::from_str(&digest).context("parse digest")
    }

    /// Pull the bytes of a layer from the registry in a stream.
    /// The `media_type` field of the [`LayerDescriptor`] can be used to determine how best to handle the content.
    ///
    /// Note: layer filters are not used by this function;
    /// this is because the layer is already filtered by the [`Registry::layers`] method,
    /// so this only matters if you create your own [`LayerDescriptor`] and pass it to this function.
    ///
    /// ## Layers explanation
    ///
    /// You can think of a layer as a "diff" (you can envision this similarly to a git diff)
    /// from the previous layer; the first layer is a "diff" from an empty layer.
    ///
    /// Each diff contains zero or more changes; each change is one of the below:
    /// - A file is added.
    /// - A file is removed.
    /// - A file is modified.
    pub async fn pull_layer(
        &self,
        layer: &Layer,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        self.pull_layer_internal(layer)
            .await
            .map(|stream| stream.map(|chunk| chunk.context("read chunk")).boxed())
    }

    async fn pull_layer_internal(&self, layer: &Layer) -> Result<impl Stream<Item = Chunk>> {
        let oci_layer = OciDescriptor::from(layer);
        self.client
            .pull_blob_stream(&self.reference, &oci_layer)
            .await
            .context("initiate stream")
            .map(|layer| layer.stream)
    }

    /// Enumerate files in a layer.
    #[tracing::instrument]
    pub async fn list_files(&self, layer: &Layer) -> Result<Vec<String>> {
        let stream = self.pull_layer_internal(layer).await?;

        // Applying the layer requires interpreting the layer's media type.
        match &layer.media_type {
            // Standard OCI layers.
            LayerMediaType::Oci(flags) => {
                // Foreign layers are skipped, as they would if you ran `docker pull`.
                // This causes an extra iteration over the flags for layers that aren't foreign,
                // but the flag count is small and this saves us the complexity of setting up layer transforms
                // and then discarding them if this flag is encountered.
                if flags.contains(&LayerMediaTypeFlag::Foreign) {
                    warn!("skip: foreign layer");
                    return Ok(Vec::new());
                }

                // The vast majority of the time (maybe even all the time), the layer only has zero or one flags.
                // Meanwhile, `transform::sequence` forces the streams into dynamic dispatch, imposing extra overhead.
                // This match allows us to specialize the stream based on the most common cases,
                // while still supporting arbitrary flags.
                match flags.as_slice() {
                    // No flags; this means the layer is uncompressed.
                    [] => enumerate_tarball(stream).await,

                    // The layer is compressed with zstd.
                    [LayerMediaTypeFlag::Zstd] => {
                        let stream = transform::zstd(stream);
                        enumerate_tarball(stream).await
                    }

                    // The layer is compressed with gzip.
                    [LayerMediaTypeFlag::Gzip] => {
                        let stream = transform::gzip(stream);
                        enumerate_tarball(stream).await
                    }

                    // The layer has a more complicated set of flags.
                    // For this, we fall back to the generic sequence operator.
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
    ///
    /// As such the following edge cases are handled as follows:
    /// - Foreign layers are treated as no-ops, as they would if you ran `docker pull`.
    /// - Standard layers are applied as normal.
    ///
    /// If you wish to customize the behavior, use [`Registry::pull_layer`] directly instead.
    ///
    /// ## Application order
    ///
    /// This method performs the following steps:
    /// 1. Downloads the specified layer from the registry.
    /// 2. Applies the layer diff to the specified path on disk.
    ///
    /// When applying multiple layers, it's important to apply them in order,
    /// and to apply them to a consistent location on disk.
    ///
    /// It is safe to apply each layer to a fresh directory if a separate directory per layer is desired:
    /// the only sticking point for this case is removed files,
    /// and this function simply skips removing files that don't exist.
    ///
    /// ## Layers explanation
    ///
    /// You can think of a layer as a "diff" (you can envision this similarly to a git diff)
    /// from the previous layer; the first layer is a "diff" from an empty layer.
    ///
    /// Each diff contains zero or more changes; each change is one of the below:
    /// - A file is added.
    /// - A file is removed.
    /// - A file is modified.
    ///
    /// More information: https://github.com/opencontainers/image-spec/blob/main/layer.md
    //
    // A future improvement would be to support downloading layers concurrently,
    // then still applying them serially. Since network transfer is the slowest part of this process,
    // this would speed up the overall process.
    #[tracing::instrument]
    pub async fn apply_layer(&self, layer: &Layer, output: &Path) -> Result<()> {
        let stream = self.pull_layer_internal(layer).await?;

        // Applying the layer requires interpreting the layer's media type.
        match &layer.media_type {
            // Standard OCI layers.
            LayerMediaType::Oci(flags) => {
                // Foreign layers are skipped, as they would if you ran `docker pull`.
                // This causes an extra iteration over the flags for layers that aren't foreign,
                // but the flag count is small and this saves us the complexity of setting up layer transforms
                // and then discarding them if this flag is encountered.
                if flags.contains(&LayerMediaTypeFlag::Foreign) {
                    warn!("skip: foreign layer");
                    return Ok(());
                }

                // The vast majority of the time (maybe even all the time), the layer only has zero or one flags.
                // Meanwhile, `transform::sequence` forces the streams into dynamic dispatch, imposing extra overhead.
                // This match allows us to specialize the stream based on the most common cases,
                // while still supporting arbitrary flags.
                match flags.as_slice() {
                    // No flags; this means the layer is uncompressed.
                    [] => apply_tarball(&self.file_filters, stream, output).await,

                    // The layer is compressed with zstd.
                    [LayerMediaTypeFlag::Zstd] => {
                        let stream = transform::zstd(stream);
                        apply_tarball(&self.file_filters, stream, output).await
                    }

                    // The layer is compressed with gzip.
                    [LayerMediaTypeFlag::Gzip] => {
                        let stream = transform::gzip(stream);
                        apply_tarball(&self.file_filters, stream, output).await
                    }

                    // The layer has a more complicated set of flags.
                    // For this, we fall back to the generic sequence operator.
                    _ => {
                        let stream = transform::sequence(stream, flags);
                        apply_tarball(&self.file_filters, stream, output).await
                    }
                }
            }
        }
    }

    /// Normalize an OCI layer into a plain tarball layer.
    /// This is intended to support FOSSA CLI's needs; see the [`fossacli`] module docs for details.
    ///
    /// The intention of this method is that when it is run for each layer in an image in order it is equivalent
    /// to the functionality you'd get by running `docker pull`, `docker save`, and viewing the patch sets directly.
    ///
    /// The twist though is that OCI servers can wrap various kinds of compression around tarballs;
    /// this method flattens them all down into plain uncompressed `.tar` files.
    ///
    /// As such the following edge cases are handled as follows:
    /// - Standard layers are applied as normal, except that they are re-encoded to plain uncompressed tarballs.
    /// - Foreign layers are treated as no-ops, as they would if you ran `docker pull`.
    ///   These are emitted as `None`.
    /// - File path filters are ignored.
    ///   this is a consequence of the fact that we don't actually unpack and read the tarball.
    ///   For the purposes of FOSSA CLI interop this is fine as the `reexport` subcommand doesn't even support filters,
    ///   but if we ever want to make this work for more than just that we'll need to re-evaluate.
    #[tracing::instrument]
    pub async fn layer_plain_tarball(&self, layer: &Layer) -> Result<Option<TempFile>> {
        let stream = self.pull_layer_internal(layer).await?;

        // Applying the layer requires interpreting the layer's media type.
        match &layer.media_type {
            // Standard OCI layers.
            LayerMediaType::Oci(flags) => {
                // Foreign layers are skipped, as they would if you ran `docker pull`.
                // This causes an extra iteration over the flags for layers that aren't foreign,
                // but the flag count is small and this saves us the complexity of setting up layer transforms
                // and then discarding them if this flag is encountered.
                if flags.contains(&LayerMediaTypeFlag::Foreign) {
                    warn!("skip: foreign layer");
                    return Ok(None);
                }

                // The vast majority of the time (maybe even all the time), the layer only has zero or one flags.
                // Meanwhile, `transform::sequence` forces the streams into dynamic dispatch, imposing extra overhead.
                // This match allows us to specialize the stream based on the most common cases,
                // while still supporting arbitrary flags.
                Ok(Some(match flags.as_slice() {
                    // No flags; this means the layer is already uncompressed.
                    [] => collect_tmp(stream).await?,

                    // The layer is compressed with zstd.
                    [LayerMediaTypeFlag::Zstd] => transform::zstd(stream).pipe(collect_tmp).await?,

                    // The layer is compressed with gzip.
                    [LayerMediaTypeFlag::Gzip] => transform::gzip(stream).pipe(collect_tmp).await?,

                    // The layer has a more complicated set of flags.
                    // For this, we fall back to the generic sequence operator.
                    _ => transform::sequence(stream, flags).pipe(collect_tmp).await?,
                }))
            }
        }
    }
}

impl Source for Registry {
    async fn digest(&self) -> Result<Digest> {
        self.digest().await
    }

    async fn name(&self) -> Result<String> {
        Ok(self.original.name.clone())
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

impl From<&Reference> for OciReference {
    fn from(reference: &Reference) -> Self {
        match &reference.version {
            Version::Tag(tag) => {
                Self::with_tag(reference.host.clone(), reference.repository(), tag.clone())
            }
            Version::Digest(digest) => Self::with_digest(
                reference.host.clone(),
                reference.repository(),
                digest.to_string(),
            ),
        }
    }
}

impl From<Layer> for OciDescriptor {
    fn from(layer: Layer) -> Self {
        Self {
            digest: layer.digest.to_string(),
            media_type: layer.media_type.to_string(),
            size: layer.size,
            ..Default::default()
        }
    }
}

impl From<&Layer> for OciDescriptor {
    fn from(layer: &Layer) -> Self {
        layer.clone().into()
    }
}

impl TryFrom<OciDescriptor> for Layer {
    type Error = color_eyre::Report;

    fn try_from(value: OciDescriptor) -> Result<Self, Self::Error> {
        Ok(Self {
            digest: Digest::from_str(&value.digest).context("parse digest")?,
            media_type: LayerMediaType::from_str(&value.media_type).context("parse media type")?,
            size: value.size,
        })
    }
}

impl From<Authentication> for RegistryAuth {
    fn from(auth: Authentication) -> Self {
        match auth {
            Authentication::None => RegistryAuth::Anonymous,
            Authentication::Basic { username, password } => RegistryAuth::Basic(username, password),
        }
    }
}

impl FilterMatch<&Layer> for Filter {
    fn matches(&self, value: &Layer) -> bool {
        self.matches(&value.digest.to_string())
    }
}

impl FilterMatch<&OciDescriptor> for Filter {
    fn matches(&self, value: &OciDescriptor) -> bool {
        self.matches(&value.digest)
    }
}

impl FilterMatch<&PathBuf> for Filter {
    fn matches(&self, value: &PathBuf) -> bool {
        self.matches(value.to_string_lossy())
    }
}

fn client(platform: Option<Platform>) -> Client {
    let mut config = ClientConfig::default();
    config.platform_resolver = match platform {
        Some(platform) => Some(Box::new(target_platform_resolver(platform))),
        None => Some(Box::new(current_platform_resolver)),
    };
    Client::new(config)
}

fn target_platform_resolver(target: Platform) -> impl Fn(&[ImageIndexEntry]) -> Option<String> {
    move |entries: &[ImageIndexEntry]| {
        entries
            .iter()
            .find(|entry| {
                entry.platform.as_ref().map_or(false, |platform| {
                    platform.os == target.os && platform.architecture == target.architecture
                })
            })
            .map(|entry| entry.digest.clone())
    }
}

fn current_platform_resolver(entries: &[ImageIndexEntry]) -> Option<String> {
    let current_os = go_os();
    let current_arch = go_arch();
    let linux = Platform::LINUX;
    let amd64 = Platform::AMD64;
    entries
        .iter()
        .priority_find(|entry| match entry.platform.as_ref() {
            None => 0,
            Some(p) if p.os == current_os && p.architecture == current_arch => 1,
            Some(p) if p.os == linux && p.architecture == current_arch => 2,
            Some(p) if p.os == linux && p.architecture == amd64 => 3,
            _ => 4,
        })
        .map(|entry| entry.digest.clone())
}

/// Returns the current OS as a string that matches a `GOOS` constant.
/// This is required because the OCI spec requires the OS to be a valid GOOS value.
// If you get a compile error here, you need to add a new `cfg` branch for your platform.
// Valid GOOS values may be gathered from here: https://go.dev/doc/install/source#environment
const fn go_os() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "linux"
    }
    #[cfg(target_os = "macos")]
    {
        "darwin"
    }
    #[cfg(target_os = "windows")]
    {
        "windows"
    }
}

/// Returns the current architecture as a string that matches a `GOARCH` constant.
/// This is required because the OCI spec requires the architecture to be a valid GOARCH value.
// If you get a compile error here, you need to add a new `cfg` branch for your platform.
// Valid GOARCH values may be gathered from here: https://go.dev/doc/install/source#environment
const fn go_arch() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        "amd64"
    }
    #[cfg(target_arch = "aarch64")]
    {
        "arm64"
    }
}
