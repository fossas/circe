//! Interacts with remote OCI registries.

use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use async_tempfile::TempFile;
use bytes::Bytes;
use color_eyre::eyre::{Context, OptionExt, Result};
use derive_more::Debug;
use futures_lite::{Stream, StreamExt};
use oci_client::{
    client::ClientConfig,
    manifest::{ImageIndexEntry, OciDescriptor},
    secrets::RegistryAuth,
    Client, Reference as OciReference, RegistryOperation,
};
use os_str_bytes::OsStrBytesExt;
use tap::Pipe;
use tokio::io::{AsyncRead, AsyncWriteExt, BufWriter};
use tokio_tar::{Archive, Entry};
use tokio_util::io::StreamReader;
use tracing::{debug, warn};

use crate::{
    ext::PriorityFind,
    transform::{self, Chunk},
    Authentication, Digest, Filter, FilterMatch, Filters, Layer, LayerMediaType,
    LayerMediaTypeFlag, Platform, Reference, Version,
};

/// Unwrap a value, logging an error and performing the provided action if it fails.
macro_rules! unwrap_warn {
    ($expr:expr, $action:expr) => {
        unwrap_warn!($expr, $action,)
    };
    ($expr:expr, $action:expr, $($msg:tt)*) => {
        match $expr {
            Ok(value) => value,
            Err(e) => {
                tracing::warn!(error = ?e, $($msg)*);
                $action;
            }
        }
    };
}

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
    pub async fn pull_layer(&self, layer: &Layer) -> Result<impl Stream<Item = Result<Bytes>>> {
        self.pull_layer_internal(layer)
            .await
            .map(|stream| stream.map(|chunk| chunk.context("read chunk")))
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

/// Sink the stream into a temporary file.
async fn collect_tmp(mut stream: impl Stream<Item = Chunk> + Unpin) -> Result<TempFile> {
    let file = TempFile::new().await.context("create temp file")?;
    let mut writer = BufWriter::new(file);

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("read chunk")?;
        writer.write_all(&chunk).await.context("write chunk")?;
    }
    writer.flush().await.context("flush writer")?;

    let file = writer.into_inner();
    file.sync_all().await.context("sync file")?;
    Ok(file)
}

/// Apply files in the tarball to a location on disk.
async fn apply_tarball(
    path_filters: &Filters,
    stream: impl Stream<Item = Chunk> + Unpin,
    output: &Path,
) -> Result<()> {
    let reader = StreamReader::new(stream);
    let mut archive = Archive::new(reader);
    let mut entries = archive.entries().context("read entries from tar")?;

    // Future improvement: the OCI spec guarantees that paths will not repeat within the same layer,
    // so we could concurrently read files and apply them to disk.
    // The overall archive is streaming so we'd need to buffer the entries,
    // but assuming disk is the bottleneck this might speed up the process significantly.
    // We could also of course write the tar to disk and then extract it concurrently
    // without buffering- maybe we could read the tar entries while streaming to disk,
    // and then divide them among workers that apply them to disk concurrently?
    while let Some(entry) = entries.next().await {
        let mut entry = unwrap_warn!(entry, continue, "read entry");
        let path = unwrap_warn!(entry.path(), continue, "read entry path");

        // Paths inside the container are relative to the root of the container;
        // we need to convert them to be relative to the output directory.
        let path = output.join(path);

        if !path_filters.matches(&path) {
            debug!(?path, "skip: path filter");
            continue;
        }

        // Whiteout files delete the file from the filesystem.
        if let Some(path) = is_whiteout(&path) {
            unwrap_warn!(
                tokio::fs::remove_file(&path).await,
                continue,
                "whiteout: {path:?}"
            );
            debug!(?path, "whiteout");
            continue;
        }

        // The tar library mostly handles symlinks properly, but still allows them to link to absolute paths.
        // This doesn't technically break anything from a security standpoint, but might for analysis.
        // Intercept its handling of absolute symlinks to handle this case.
        if entry.header().entry_type().is_symlink() {
            let handled = unwrap_warn!(
                safe_symlink(&entry, output).await,
                continue,
                "create symlink {path:?}"
            );

            // But if the function didn't handle it, fall back to the default behavior.
            if handled {
                continue;
            }
        }

        // Future improvement: symlinks are unpacked with the same destination as written in the actual container;
        // this means e.g. they can link to files outside of the output directory
        // (the example case I found was in `usr/bin`, linking to `/bin/`).
        // I don't _think_ this matters for now given how we're using this today, but it's technically incorrect.
        // To fix this we need to re-implement the logic in `unpack_in` to rewrite symlink destinations.

        // Otherwise, apply the file as normal.
        // Both _new_ and _changed_ files are handled the same way:
        // the layer contains the entire file content, so we just overwrite the file.
        if !unwrap_warn!(entry.unpack_in(output).await, continue, "unpack {path:?}") {
            warn!(?path, "skip: tried to write outside of output directory");
            continue;
        }

        debug!(?path, "apply");
    }

    Ok(())
}

/// Enumerate files in a tarball.
async fn enumerate_tarball(stream: impl Stream<Item = Chunk> + Unpin) -> Result<Vec<String>> {
    let reader = StreamReader::new(stream);
    let mut archive = Archive::new(reader);
    let mut entries = archive.entries().context("read entries from tar")?;

    let mut files = Vec::new();
    while let Some(entry) = entries.next().await {
        let entry = unwrap_warn!(entry, continue, "read entry");
        let path = unwrap_warn!(entry.path(), continue, "read entry path");
        debug!(?path, "enumerate");
        files.push(path.to_string_lossy().to_string());
    }

    Ok(files)
}

/// Special handling for symlinks that link to an absolute path.
/// It effectively forces the destination into a path relative to the output directory.
///
/// Returns true if the symlink was handled;
/// false if the symlink should fall back to standard handling from `async_tar`.
async fn safe_symlink<R: AsyncRead + Unpin>(entry: &Entry<R>, dir: &Path) -> Result<bool> {
    let header = entry.header();
    let kind = header.entry_type();
    if !kind.is_symlink() {
        return Ok(false);
    }

    let link = entry.path().context("read symlink source")?;
    let target = header
        .link_name()
        .context("read symlink target")?
        .ok_or_eyre("no symlink target")?;

    // If the target is relative, we should let `async_tar` handle it;
    // this function only needs to intercept absolute symlinks.
    if !target.is_absolute() {
        return Ok(false);
    }

    let safe_link = dir.join(&link);
    let safe_target = dir.join(strip_root(&target));

    let rel_target = compute_symlink_target(&safe_link, &safe_target)
        .with_context(|| format!("compute relative path from {safe_link:?} to {safe_target:?}"))?;
    debug!(
        ?link,
        ?target,
        ?safe_link,
        ?safe_target,
        ?rel_target,
        "create symlink"
    );

    if let Some(parent) = safe_link.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("create parent directory")?;
    }

    symlink(&rel_target, &safe_link)
        .await
        .map(|_| true)
        .with_context(|| {
            format!("create symlink from {safe_link:?} to {safe_target:?} as {rel_target:?}")
        })
}

fn compute_symlink_target(src: &Path, dst: &Path) -> Result<PathBuf> {
    let common_prefix = src
        .components()
        .zip(dst.components())
        .by_ref()
        .take_while(|(src, dst)| src == dst)
        .map(|(src, _)| src)
        .collect::<PathBuf>();

    let src_rel = src
        .strip_prefix(&common_prefix)
        .context("strip common prefix from src")?;
    let dst_rel = dst
        .strip_prefix(&common_prefix)
        .context("strip common prefix from dst")?;

    // `bridge` is the path from the source to the common prefix.
    let bridge = src_rel
        .components()
        .skip(1)
        .map(|_| "..")
        .collect::<PathBuf>();
    let rel = bridge.join(dst_rel);

    // `.` indicates that the source and destination are the same file.
    if rel.to_string_lossy().is_empty() {
        Ok(PathBuf::from("."))
    } else {
        Ok(rel)
    }
}

/// Strips any root and prefix from a path, if they exist.
fn strip_root(path: impl AsRef<Path>) -> PathBuf {
    path.as_ref()
        .components()
        .filter(|c| match c {
            std::path::Component::Prefix(_) => false,
            std::path::Component::RootDir => false,
            _ => true,
        })
        .pipe(PathBuf::from_iter)
}

#[cfg(windows)]
async fn symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    let (src, dst) = (src.to_owned(), dst.to_owned());
    tokio::task::spawn_blocking(|| std::os::windows::fs::symlink_file(src, dst))
        .await
        .expect("join tokio task")
}

#[cfg(any(unix, target_os = "redox"))]
async fn symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    tokio::fs::symlink(src, dst).await
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
    use pretty_assertions::assert_eq;
    use simple_test_case::test_case;

    #[test]
    fn test_is_whiteout() {
        assert_eq!(None, is_whiteout(Path::new("foo")));
        assert_eq!(
            Some(PathBuf::from("foo")),
            is_whiteout(Path::new(".wh.foo")),
        );
    }

    #[test_case(Path::new("/a/b/c"), Path::new("/a/b/d/e/f"), PathBuf::from("d/e/f"); "one_level")]
    #[test_case(Path::new("/usr/local/bin/ls"), Path::new("/bin/ls"), PathBuf::from("../../../bin/ls"); "usr_local_bin_to_bin")]
    #[test_case(Path::new("/usr/local/bin/ls"), Path::new("/usr/bin/ls"), PathBuf::from("../../bin/ls"); "usr_local_bin_to_usr_bin")]
    #[test_case(Path::new("/usr/local/bin/ls"), Path::new("/usr/local/bin/ls"), PathBuf::from("."); "same_file")]
    #[test_case(Path::new("/usr/local/bin/eza"), Path::new("/usr/local/bin/ls"), PathBuf::from("ls"); "same_dir")]
    #[tokio::test]
    async fn compute_symlink_target(src: &Path, dst: &Path, expected: PathBuf) -> Result<()> {
        let relative = compute_symlink_target(src, dst)?;
        pretty_assertions::assert_eq!(expected, relative);
        Ok(())
    }
}
