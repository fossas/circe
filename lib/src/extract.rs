use std::path::{Path, PathBuf};

use crate::{Digest, Layer, Reference, Source};
use bon::Builder;
use color_eyre::{
    eyre::{bail, Context, Error},
    Result,
};
use futures_lite::{stream, StreamExt};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use tap::Pipe;
use tracing::info;

/// Report containing details about the extracted container image.
#[derive(Debug, Serialize, Builder)]
pub struct Report {
    /// The original reference requested when extracting the image.
    #[builder(into)]
    pub reference: Reference,

    /// The repository name of the image.
    #[builder(into)]
    pub name: String,

    /// The content-addressable digest of the image.
    #[builder(into)]
    pub digest: String,

    /// The extracted layers and their corresponding filesystem paths.
    ///
    /// When multiple layer digests point to the same directory path,
    /// it indicates those layers were squashed together in their application order.
    #[builder(into)]
    pub layers: Vec<(Digest, PathBuf)>,
}

impl Report {
    /// The standard name for the report file.
    // Note: if this changes, make sure to update the `extract` CLI documentation.
    pub const FILENAME: &'static str = "image.json";

    /// Write the report to its standard location in the output directory.
    pub async fn write(&self, output: &Path) -> Result<()> {
        let path = output.join(Self::FILENAME);
        tokio::fs::write(&path, self.render()?)
            .await
            .context("write report")
    }

    /// Render the report to a string.
    pub fn render(&self) -> Result<String> {
        serde_json::to_string_pretty(self).context("serialize report")
    }
}

/// Extraction strategy for container layers.
pub enum Strategy {
    /// Squash multiple layers into a single unified filesystem.
    ///
    /// This applies layers in sequence, with each layer's changes
    /// overlaying previous layers' contents.
    Squash(Vec<Layer>),

    /// Extract a single layer to its own directory without combining it with others.
    Separate(Layer),
}

impl IntoIterator for Strategy {
    type Item = Strategy;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        vec![self].into_iter()
    }
}

/// Extract container layers according to the specified strategies.
pub async fn extract(
    registry: &impl Source,
    output: &Path,
    strategies: impl IntoIterator<Item = Strategy>,
) -> Result<Vec<(Digest, PathBuf)>> {
    // TODO: we should be able to make these concurrent:
    // each squash needs to happen in order but the strategies
    // themselves are independent.
    stream::iter(strategies)
        .then(async |strategy| match strategy {
            Strategy::Squash(layers) => squash(registry, output, &layers).await,
            Strategy::Separate(layer) => copy(registry, output, layer).await,
        })
        .try_collect::<Vec<(Digest, PathBuf)>, Error, Vec<_>>()
        .await
        .context("apply layers")
        .map(|layers| layers.into_iter().flatten().collect::<Vec<_>>())
}

async fn squash(
    registry: &impl Source,
    output: &Path,
    layers: &[Layer],
) -> Result<Vec<(Digest, PathBuf)>> {
    let target = target_dir(output, layers).context("target dir")?;
    info!(layers = ?layers.iter().map(|l| &l.digest).collect::<Vec<_>>(), target = ?target.display(), "squash layers");

    stream::iter(layers)
        .then(async |layer| -> Result<(Digest, PathBuf)> {
            tokio::fs::create_dir_all(&target).await?;
            registry.apply_layer(layer, &target).await?;
            Ok((layer.digest.clone(), target.clone()))
        })
        .try_collect()
        .await
}

async fn copy(
    registry: &impl Source,
    output: &Path,
    layer: Layer,
) -> Result<Vec<(Digest, PathBuf)>> {
    let target = target_dir(output, [&layer]).context("target dir")?;
    info!(layer = ?layer.digest, target = ?target.display(), "copy layer");

    tokio::fs::create_dir_all(&target).await?;
    registry.apply_layer(&layer, &target).await?;
    Ok(vec![(layer.digest.clone(), target)])
}

/// Computes a directory for a set of layers to be squashed in the output directory.
///
/// If there is only one layer, the directory name is the digest of the layer.
/// If there are multiple layers, the directory name is a hash of the digests of the layers.
///
/// Each variant has a short prefix; this is to handle the fact that it's technically possible
/// to have layer digests clash. This really shouldn't ever happen, but better to be safe.
/// Note that this is considered an implementation detail; the presence or stability of this identifier
/// is not part of the public contract of this application.
fn target_dir<'a>(
    output: &Path,
    layers: impl IntoIterator<Item = &'a Layer> + 'a,
) -> Result<PathBuf> {
    match layers.into_iter().collect::<Vec<_>>().as_slice() {
        [] => bail!("empty layers"),
        [layer] => format!("si_{}", layer.digest.as_hex()),
        layers => layers
            .iter()
            .fold(Sha256::new(), |mut hasher, layer| {
                hasher.update(&layer.digest.hash);
                hasher
            })
            .finalize()
            .to_vec()
            .pipe_ref(hex::encode)
            .pipe(|hash| format!("sq_{hash}")),
    }
    .pipe(|name| output.join(name))
    .pipe(Ok)
}
