//! Interacts with remote OCI registries.

use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

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
use os_str_bytes::OsStrBytesExt;
use tokio_tar::Archive;
use tokio_util::io::StreamReader;
use tracing::{debug, warn};

use crate::{
    ext::PriorityFind,
    transform::{self, Chunk},
    Digest, LayerDescriptor, LayerMediaType, LayerMediaTypeFlag, Platform, Reference, Version,
};

/// Each instance is a unique view of remote registry for a specific [`Platform`] and [`Reference`].
/// The intention here is to better support chained methods like "pull list of layers" and then "apply each layer to disk".
// Note: internal fields aren't public because we don't want the caller to be able to mutate the internal state between method calls.
#[derive(Debug, Clone)]
pub struct Registry {
    /// The OCI reference, used by the underlying client.
    reference: OciReference,

    /// Authentication information for the registry.
    auth: RegistryAuth,

    /// The client used to interact with the registry.
    #[debug(skip)]
    client: Client,
}

#[bon::bon]
impl Registry {
    /// Create a new registry for a specific platform and reference.
    #[builder]
    pub async fn new(platform: Option<Platform>, reference: Reference) -> Result<Self> {
        let client = client(platform.clone());
        let reference = OciReference::from(&reference);

        // Future improvement: support authentication.
        let auth = RegistryAuth::Anonymous;

        client
            .auth(&reference, &auth, RegistryOperation::Pull)
            .await
            .context("authenticate to registry")?;

        Ok(Self {
            auth,
            client,
            reference,
        })
    }
}

impl Registry {
    /// Enumerate layers for a container reference in the remote registry.
    /// Layers are returned in order from the base image to the application.
    #[tracing::instrument]
    pub async fn layers(&self) -> Result<Vec<LayerDescriptor>> {
        let (manifest, _) = self
            .client
            .pull_image_manifest(&self.reference, &self.auth)
            .await
            .context("pull image manifest")?;
        manifest
            .layers
            .into_iter()
            .map(LayerDescriptor::try_from)
            .collect()
    }

    /// Pull the bytes of a layer from the registry in a stream.
    /// The `media_type` field of the [`LayerDescriptor`] can be used to determine how best to handle the content.
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
        layer: &LayerDescriptor,
    ) -> Result<impl Stream<Item = Result<Bytes>>> {
        self.pull_layer_internal(layer)
            .await
            .map(|stream| stream.map(|chunk| chunk.context("read chunk")))
    }

    async fn pull_layer_internal(
        &self,
        layer: &LayerDescriptor,
    ) -> Result<impl Stream<Item = Chunk>> {
        let oci_layer = OciDescriptor::from(layer);
        self.client
            .pull_blob_stream(&self.reference, &oci_layer)
            .await
            .context("initiate stream")
            .map(|layer| layer.stream)
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
    // #[tracing::instrument]
    pub async fn apply_layer(&self, layer: &LayerDescriptor, output: &Path) -> Result<()> {
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
                    [] => apply_tarball(stream, output).await,

                    // The layer is compressed with zstd.
                    [LayerMediaTypeFlag::Zstd] => {
                        let stream = transform::zstd(stream);
                        apply_tarball(stream, output).await
                    }

                    // The layer is compressed with gzip.
                    [LayerMediaTypeFlag::Gzip] => {
                        let stream = transform::gzip(stream);
                        apply_tarball(stream, output).await
                    }

                    // The layer has a more complicated set of flags.
                    // For this, we fall back to the generic sequence operator.
                    _ => {
                        let stream = transform::sequence(stream, flags);
                        apply_tarball(stream, output).await
                    }
                }
            }
        }
    }
}

async fn apply_tarball(stream: impl Stream<Item = Chunk> + Unpin, output: &Path) -> Result<()> {
    let reader = StreamReader::new(stream);
    let mut archive = Archive::new(reader);
    let mut entries = archive.entries().context("read entries from tar")?;

    /// Unwrap a value, logging an error and continuing the loop if it fails.
    macro_rules! unwrap_warn {
        ($expr:expr) => {
            unwrap_warn!($expr,);
        };
        ($expr:expr, $($msg:tt)*) => {
            match $expr {
                Ok(value) => value,
                Err(e) => {
                    tracing::warn!(error = ?e, $($msg)*);
                    continue;
                }
            }
        };
    }

    // Future improvement: the OCI spec guarantees that paths will not repeat within the same layer,
    // so we could concurrently read files and apply them to disk.
    // The overall archive is streaming so we'd need to buffer the entries,
    // but assuming disk is the bottleneck this might speed up the process significantly.
    // We could also of course write the tar to disk and then extract it concurrently
    // without buffering- maybe we could read the tar entries while streaming to disk,
    // and then divide them among workers that apply them to disk concurrently?
    while let Some(entry) = entries.next().await {
        let mut entry = unwrap_warn!(entry, "read entry");
        let path = unwrap_warn!(entry.path(), "read entry path");

        // Paths inside the container are relative to the root of the container;
        // we need to convert them to be relative to the output directory.
        let path = output.join(path);

        // Whiteout files delete the file from the filesystem.
        if let Some(path) = is_whiteout(&path) {
            unwrap_warn!(tokio::fs::remove_file(&path).await, "whiteout: {path:?}");
            debug!(?path, "whiteout");
            continue;
        }

        // Future improvement: symlinks are unpacked with the same destination as written in the actual container;
        // this means e.g. they can link to files outside of the output directory
        // (the example case I found was in `usr/bin`, linking to `/bin/`).
        // I don't _think_ this matters for now given how we're using this today, but it's technically incorrect.
        // To fix this we need to re-implement the logic in `unpack_in` to rewrite symlink destinations.

        // Otherwise, apply the file as normal.
        // Both _new_ and _changed_ files are handled the same way:
        // the layer contains the entire file content, so we just overwrite the file.
        if !unwrap_warn!(entry.unpack_in(output).await, "unpack {path:?}") {
            warn!(?path, "skip: tried to write outside of output directory");
            continue;
        }

        debug!(?path, "apply");
    }

    Ok(())
}

impl From<&Reference> for OciReference {
    fn from(reference: &Reference) -> Self {
        match &reference.version {
            Version::Tag(tag) => Self::with_tag(
                reference.host.clone(),
                reference.repository.clone(),
                tag.clone(),
            ),
            Version::Digest(digest) => Self::with_digest(
                reference.host.clone(),
                reference.repository.clone(),
                digest.to_string(),
            ),
        }
    }
}

impl From<LayerDescriptor> for OciDescriptor {
    fn from(layer: LayerDescriptor) -> Self {
        Self {
            digest: layer.digest.to_string(),
            media_type: layer.media_type.to_string(),
            size: layer.size,
            ..Default::default()
        }
    }
}

impl From<&LayerDescriptor> for OciDescriptor {
    fn from(layer: &LayerDescriptor) -> Self {
        layer.clone().into()
    }
}

impl TryFrom<OciDescriptor> for LayerDescriptor {
    type Error = color_eyre::Report;

    fn try_from(value: OciDescriptor) -> Result<Self, Self::Error> {
        Ok(Self {
            digest: Digest::from_str(&value.digest).context("parse digest")?,
            media_type: LayerMediaType::from_str(&value.media_type).context("parse media type")?,
            size: value.size,
        })
    }
}

/// Returns the path to the file that would be deleted by a whiteout file, if the path is a whiteout file.
/// If the path is not a whiteout file, returns `None`.
fn is_whiteout(path: &Path) -> Option<PathBuf> {
    const WHITEOUT_PREFIX: &str = ".wh.";

    // If the file doesn't have a name, it's not a whiteout file.
    // Similarly if it doesn't have the prefix, it's also not a whiteout file.
    let name = path.file_name()?.strip_prefix(WHITEOUT_PREFIX)?;
    Some(match path.parent() {
        Some(parent) => PathBuf::from(parent).join(name),
        None => PathBuf::from(name),
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_whiteout() {
        assert_eq!(is_whiteout(Path::new("foo")), None);
        assert_eq!(
            is_whiteout(Path::new(".wh.foo")),
            Some(PathBuf::from("foo"))
        );
    }
}
