//! Module for FOSSA CLI container compatibility support
//!
//! This module enables Circe to bridge between remote OCI container formats
//! and the tarball format expected by FOSSA CLI's container scanning system.
//!
//! FOSSA CLI processes container images as tarballs during analysis.
//! This module provides the necessary types and conversion utilities
//! to transform container images into the specific tarball format
//! that FOSSA CLI can parse and analyze.
//!
//! For reference implementations, see the test data in `/lib/tests/it/testdata/fossacli`
//! which contains example tarballs that match FOSSA CLI's expected format.
//! These examples are also available in the `fossaeng` DockerHub account,
//! though the vendored examples in this repo are more reliable as reference
//! implementations since they are not subject to Docker platform changes.

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

/// Container image configuration for FOSSA CLI.
#[derive(Debug, Clone, Serialize)]
pub struct Image {
    /// The root filesystem definition containing the container's layer information.
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

/// Root filesystem structure for a container image.
///
/// This defines how container layers are organized in the filesystem.
/// FOSSA CLI uses this structure to understand which layers make up
/// the container and in what order they should be applied.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum RootFs {
    /// A layered filesystem structure composed of multiple layer diff IDs.
    ///
    /// This is the only rootfs type supported by FOSSA CLI's container analyzer.
    /// The layers must be listed in application order (base layer first).
    Layers {
        /// The content-addressable digests of each layer in application order.
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

/// Serializes a value to JSON and writes it to a temporary file.
async fn write_serialized_tempfile<T: Serialize>(value: &T) -> Result<(TempFile, String)> {
    let mut file = TempFile::new().await.context("create")?;
    let value = serde_json::to_string_pretty(&value).context("serialize")?;
    file.write_all(value.as_bytes()).await.context("write")?;
    file.sync_all().await.context("sync")?;
    Ok((file, value))
}
