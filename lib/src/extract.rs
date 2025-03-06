use std::path::{Path, PathBuf};

use crate::{registry::Registry, Digest, Layer, Reference};
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

/// Reports details about the image that was extracted.
#[derive(Debug, Serialize, Builder)]
pub struct Report {
    /// The orginal requested reference of the image that was extracted.
    #[builder(into)]
    pub reference: Reference,

    /// The name of the image.
    #[builder(into)]
    pub name: String,

    /// The digest of the image.
    #[builder(into)]
    pub digest: String,

    /// The layers that were extracted, and the paths into which they were extracted.
    ///
    /// If multiple layer digests point to the same directory,
    /// this means they were squashed in the order indicated.
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

/// The strategy used to extract one or more layers.
pub enum Strategy {
    /// The indicated layers are squashed into a single layer.
    Squash(Vec<Layer>),

    /// The layer is extracted as-is.
    Separate(Layer),
}

impl IntoIterator for Strategy {
    type Item = Strategy;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        vec![self].into_iter()
    }
}

/// Extract the layers.
pub async fn extract(
    registry: &Registry,
    output: &PathBuf,
    strategies: impl IntoIterator<Item = Strategy>,
) -> Result<Report> {
    let digest = registry.digest().await.context("fetch digest")?;

    // TODO: we should be able to make these concurrent:
    // each squash needs to happen in order but the strategies
    // themselves are independent.
    let layers = stream::iter(strategies)
        .then(async |strategy| match strategy {
            Strategy::Squash(layers) => squash(registry, output, &layers).await,
            Strategy::Separate(layer) => copy(registry, output, layer).await,
        })
        .try_collect::<Vec<(Digest, PathBuf)>, Error, Vec<_>>()
        .await
        .context("apply layers")?
        .pipe(|layers| layers.into_iter().flatten().collect::<Vec<_>>());

    Report::builder()
        .name(registry.original.name().to_string())
        .reference(registry.original.clone())
        .digest(digest.to_string())
        .layers(layers)
        .build()
        .pipe(Ok)
}

async fn squash(
    registry: &Registry,
    output: &PathBuf,
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
    registry: &Registry,
    output: &PathBuf,
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
