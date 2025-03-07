//! Circe is intended to support FOSSA CLI in its ability to pull images from remote OCI hosts that use a different
//! container format than the one FOSSA CLI is built to support.
//!
//! Meanwhile FOSSA CLI has been built with the assumption that the tarball is the baseline unit
//! of container scanning; all operations end with "... and then make it a tarball and scan it".
//! Untangling this and turning it into "scan the contents of a directory" is a larger lift
//! than that for which this project currently has budget.
//!
//! As such, this module supports Circe being able to work around this by becoming a "middle layer".
//! I've done my best to reconstruct the functionality used by FOSSA CLI,
//! but actual tarballs used in FOSSA CLI test data are also vendored at `/lib/tests/it/testdata/fossacli`;
//! unpacking them and looking at the content will likely be instructive if you're working on this.
//!
//! These tarballs have also been pushed remotely in the `fossaeng` dockerhub account,
//! but note that the actual tarballs vendored into the repo are more accurate since they're "frozen in time"
//! and are therefore not subject to docker drift.

use std::path::PathBuf;

use async_tempfile::TempFile;
use bon::Builder;
use color_eyre::{eyre::Context, Result};
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::Digest;

/// The manifest for a tarball image.
///
/// Corresponds to the FOSSA CLI `ManifestJson` type:
/// https://github.com/fossas/fossa-cli/blob/0fc322a0e76e6fb6f78c0f3c13d6166d183ef830/src/Container/Docker/Manifest.hs#L45-L46
///
/// For Circe, this is a singleton list;
/// FOSSA CLI just uses the first entry in the list:
/// https://github.com/fossas/fossa-cli/blob/0fc322a0e76e6fb6f78c0f3c13d6166d183ef830/src/Container/Docker/Manifest.hs#L81-L82
#[derive(Debug, Clone, Serialize)]
pub struct Manifest(Vec<ManifestEntry>);

impl Manifest {
    /// Create a new manifest with a single entry.
    pub fn singleton(entry: ManifestEntry) -> Self {
        Self(vec![entry])
    }

    /// Build the target filename for the manifest.
    pub fn filename() -> PathBuf {
        PathBuf::from("manifest.json")
    }

    /// Write the manifest to a temporary file.
    pub async fn write_tempfile(&self) -> Result<(TempFile, String)> {
        write_serialized_tempfile(self).await
    }
}

/// An image entry for the tarball manifest.
///
/// Corresponds to the FOSSA CLI `ManifestJsonImageEntry` type:
/// https://github.com/fossas/fossa-cli/blob/0fc322a0e76e6fb6f78c0f3c13d6166d183ef830/src/Container/Docker/Manifest.hs#L54-L59
#[derive(Debug, Clone, Serialize, Builder)]
#[serde(rename_all = "PascalCase")]
pub struct ManifestEntry {
    /// References the path to the [`Image`] for this manifest.
    ///
    /// Must be named for the image digest; FOSSA CLI infers the image manifest from this filename:
    /// https://github.com/fossas/fossa-cli/blob/0fc322a0e76e6fb6f78c0f3c13d6166d183ef830/src/Container/Docker/Manifest.hs#L84-L87
    ///
    /// FOSSA CLI parses the [`Image`] specified in this path immediately upon selecting a [`ManifestEntry`]
    /// to represent the image: https://github.com/fossas/fossa-cli/blob/65046d8b1935a2693e6f30869afbc2efb868352e/src/Container/Tarball.hs#L70-L74
    #[builder(into)]
    config: PathBuf,

    /// References pointing to this image.
    ///
    /// Despite the naming, this supports both tags and digests:
    /// - redis:alpine
    /// - redis@sha256:1234567890
    ///
    /// For the purposes of Circe, this is a singleton list;
    /// FOSSA CLI just uses the first tag in the list to represent the tag
    /// for the overall image:
    /// - https://github.com/fossas/fossa-cli/blob/0fc322a0e76e6fb6f78c0f3c13d6166d183ef830/src/Container/Docker/Manifest.hs#L89-L92
    /// - https://github.com/fossas/fossa-cli/blob/3a003190692b66780d76210ee0fb35ac6375c8d2/src/App/Fossa/Container/Sources/DockerArchive.hs#L292-L305
    /// - https://github.com/fossas/fossa-cli/blob/3a003190692b66780d76210ee0fb35ac6375c8d2/src/App/Fossa/Container/Sources/DockerArchive.hs#L118
    #[builder(with = |tag: impl Into<String>| vec![tag.into()])]
    repo_tags: Vec<String>,

    /// Points to the filesystem changeset tars.
    ///
    /// The layers need to be in the same order as the `diff_ids` specified in the [`RootFs`]
    /// for the [`Image`] indicated by [`ManifestEntry::config`] as they are just zipped together:
    /// - https://github.com/fossas/fossa-cli/blob/65046d8b1935a2693e6f30869afbc2efb868352e/src/Container/Tarball.hs#L74
    /// - https://github.com/fossas/fossa-cli/blob/65046d8b1935a2693e6f30869afbc2efb868352e/src/Container/Tarball.hs#L140
    #[builder(with = |layers: impl IntoIterator<Item = impl Into<PathBuf>>| layers.into_iter().map(Into::into).collect())]
    layers: Vec<PathBuf>,
}

/// Describes a single image in the tarball.
#[derive(Debug, Clone, Serialize)]
pub struct Image {
    /// The rootfs of the image.
    pub rootfs: RootFs,
}

impl Image {
    /// Build the target filename for the image.
    pub fn filename(digest: &Digest) -> PathBuf {
        let digest = digest.as_hex();
        PathBuf::from(format!("{digest}.json"))
    }

    /// Write the image to a temporary file.
    pub async fn write_tempfile(&self) -> Result<(TempFile, String)> {
        write_serialized_tempfile(self).await
    }
}

impl From<RootFs> for Image {
    fn from(rootfs: RootFs) -> Self {
        Self { rootfs }
    }
}

impl From<&RootFs> for Image {
    fn from(rootfs: &RootFs) -> Self {
        rootfs.clone().into()
    }
}

/// Describes the rootfs for an image in the tarball.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum RootFs {
    /// The rootfs is a list of layers.
    /// This is the only kind of rootfs supported by FOSSA CLI.
    Layers {
        /// The diff ids of the layers.
        diff_ids: Vec<String>,
    },
}

impl RootFs {
    /// Create a new `Layers` variant from the provided diff ids.
    pub fn layers(diff_ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::Layers {
            diff_ids: diff_ids.into_iter().map(Into::into).collect(),
        }
    }
}

async fn write_serialized_tempfile<T: Serialize>(value: &T) -> Result<(TempFile, String)> {
    let mut file = TempFile::new().await.context("create")?;
    let value = serde_json::to_string_pretty(&value).context("serialize")?;
    file.write_all(value.as_bytes()).await.context("write")?;
    file.sync_all().await.context("sync")?;
    Ok((file, value))
}
