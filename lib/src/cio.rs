//! Container file system operations.

use std::{
    path::{Path, PathBuf},
    pin::Pin,
};

use async_tempfile::TempFile;
use bytes::{Bytes, BytesMut};
use color_eyre::{
    eyre::{Context, OptionExt},
    Result,
};
use futures_lite::{Stream, StreamExt};
use os_str_bytes::OsStrBytesExt;
use serde::de::DeserializeOwned;
use tap::Pipe;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufWriter};
use tokio_tar::{Archive, Entry};
use tokio_util::io::{ReaderStream, StreamReader};
use tracing::{debug, warn};

use crate::{
    transform::{self, Chunk},
    Digest, FilterMatch, Filters, Layer, LayerMediaType, LayerMediaTypeFlag,
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

/// Hash the specified file on disk.
pub async fn file_digest(path: &Path) -> Result<Digest> {
    use sha2::{Digest as _, Sha256};
    let mut hasher = Sha256::new();
    let mut file = tokio::fs::File::open(path).await.context("open file")?;
    let mut buffer = BytesMut::with_capacity(1024);
    while let Ok(n) = file.read_buf(&mut buffer).await {
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        buffer.clear();
    }

    let hash = hasher.finalize().to_vec();
    Ok(Digest::from_hash(hash))
}

/// Transform an OCI image layer (based on its media type) into its underlying tarball.
/// Foreign layers return `None`.
#[tracing::instrument(skip(stream))]
pub fn peel_layer(
    layer: &Layer,
    stream: impl Stream<Item = Chunk> + Unpin + 'static,
) -> Option<Pin<Box<dyn Stream<Item = Chunk>>>> {
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
                return None;
            }

            Some(match flags.as_slice() {
                // No flags; this means the layer is uncompressed.
                [] => Box::pin(stream),

                // The layer is compressed with zstd.
                [LayerMediaTypeFlag::Zstd] => Box::pin(transform::zstd(stream)),

                // The layer is compressed with gzip.
                [LayerMediaTypeFlag::Gzip] => Box::pin(transform::gzip(stream)),

                // The layer has a more complicated set of flags.
                // For this, we fall back to the generic sequence operator.
                _ => Box::pin(transform::sequence(stream, flags)),
            })
        }
    }
}

/// Sink the stream into a temporary file.
#[tracing::instrument(skip(stream))]
pub async fn collect_tmp<E: std::error::Error + Send + Sync + 'static>(
    mut stream: impl Stream<Item = Result<Bytes, E>> + Unpin,
) -> Result<TempFile> {
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

/// Buffer the contents of a byte stream.
/// Limited to 100MB of memory.
#[tracing::instrument(skip(stream))]
pub async fn collect_buf(stream: impl Stream<Item = Chunk> + Unpin) -> Result<Bytes> {
    let mut read = StreamReader::new(stream.take(100 * 1024 * 1024));
    let mut buf = Vec::new();
    read.read_to_end(&mut buf).await.context("read file")?;
    Ok(Bytes::from(buf))
}

/// Collect the contents of a byte stream and parse them as JSON.
/// Limited to 100MB of buffered memory; the parsed JSON object can be larger.
#[tracing::instrument(skip(stream))]
pub async fn collect_json<T: DeserializeOwned>(
    stream: impl Stream<Item = Chunk> + Unpin,
) -> Result<T> {
    let content = collect_buf(stream).await?;
    serde_json::from_slice(&content).context("parse json")
}

/// Read a the buffered contents of a specific file out of a tarball.
/// Returns the parsed contents and path of the first file for which the closure evaluates to `true`.
/// If no file is found, this function returns `None`.
#[tracing::instrument(skip(closure))]
pub async fn extract_json<T: DeserializeOwned>(
    tarball: &Path,
    closure: impl Fn(&Path) -> bool,
) -> Result<Option<T>> {
    match extract_file(tarball, closure).await? {
        Some(stream) => collect_json(stream).await.map(Some),
        None => Ok(None),
    }
}

/// Read a the buffered contents of a specific file out of a tarball.
/// Returns the contents of the first file for which the closure evaluates to `true`.
/// If no file is found, this function returns `None`.
#[tracing::instrument(skip(closure))]
pub async fn extract_file_buf(
    tarball: &Path,
    closure: impl Fn(&Path) -> bool,
) -> Result<Option<Bytes>> {
    match extract_file(tarball, closure).await? {
        Some(stream) => collect_buf(stream).await.map(Some),
        None => Ok(None),
    }
}

/// Read a the contents of a specific file out of a tarball.
/// Returns the contents of the first file for which the closure evaluates to `true`.
/// If no file is found, this function returns `None`.
#[tracing::instrument(skip(closure))]
pub async fn extract_file(
    tarball: &Path,
    closure: impl Fn(&Path) -> bool,
) -> Result<Option<impl Stream<Item = Chunk>>> {
    let archive = tokio::fs::File::open(tarball)
        .await
        .context("open docker tarball")?;

    let mut archive = Archive::new(archive);
    let mut entries = archive.entries().context("read entries")?;
    while let Some(entry) = entries.next().await {
        let entry = entry.context("read entry")?;
        let path = entry.path().context("read entry path")?;
        if !closure(&path) {
            continue;
        }

        debug!(?path, "extracting file");
        let stream = ReaderStream::new(entry);
        return Ok(Some(stream));
    }

    Ok(None)
}

/// Apply a layer diff tarball to a location on disk.
#[tracing::instrument(skip(stream))]
pub async fn apply_tarball(
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
#[tracing::instrument(skip(stream))]
pub async fn enumerate_tarball(stream: impl Stream<Item = Chunk> + Unpin) -> Result<Vec<String>> {
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
#[tracing::instrument(skip(entry))]
pub async fn safe_symlink<R: AsyncRead + Unpin>(entry: &Entry<R>, dir: &Path) -> Result<bool> {
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

/// Compute the relative path from a source to a destination.
#[tracing::instrument]
pub fn compute_symlink_target(src: &Path, dst: &Path) -> Result<PathBuf> {
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
pub fn strip_root(path: impl AsRef<Path>) -> PathBuf {
    path.as_ref()
        .components()
        .filter(|c| {
            !matches!(
                c,
                std::path::Component::Prefix(_) | std::path::Component::RootDir
            )
        })
        .pipe(PathBuf::from_iter)
}

#[cfg(windows)]
pub async fn symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    let (src, dst) = (src.to_owned(), dst.to_owned());
    tokio::task::spawn_blocking(|| std::os::windows::fs::symlink_file(src, dst))
        .await
        .expect("join tokio task")
}

#[cfg(any(unix, target_os = "redox"))]
pub async fn symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    tokio::fs::symlink(src, dst).await
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
