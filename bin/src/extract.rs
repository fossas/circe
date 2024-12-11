use circe::{registry::Registry, Platform, Reference};
use clap::{Parser, ValueEnum};
use color_eyre::eyre::{bail, Context, Result};
use std::{path::PathBuf, str::FromStr};
use tracing::info;

#[derive(Debug, Parser)]
pub struct Options {
    /// Image reference being extracted (e.g. docker.io/library/ubuntu:latest)
    #[arg(value_parser = Reference::from_str)]
    image: Reference,

    /// Directory to which the extracted contents will be written
    #[arg(default_value = ".")]
    output_dir: String,

    /// Overwrite the existing output directory if it exists.
    #[arg(long, short)]
    overwrite: bool,

    /// Platform to extract (e.g. linux/amd64)
    ///
    /// If the image is not multi-platform, this is ignored.
    /// If the image is multi-platform, this is used to select the platform to extract.
    ///
    /// If the image is multi-platform and this argument is not provided,
    /// the platform is chosen according to the following priority list:
    ///
    /// 1. The first platform-independent image
    ///
    /// 2. The current platform (if available)
    ///
    /// 3. The `linux` platform for the current architecture
    ///
    /// 4. The `linux` platform for the `amd64` architecture
    ///
    /// 5. The first platform in the image manifest
    #[arg(long, value_parser = Platform::from_str)]
    platform: Option<Platform>,

    /// How to handle layers during extraction
    #[arg(long, default_value = "squash")]
    mode: Mode,
}

#[derive(Copy, Clone, Debug, Default, ValueEnum)]
pub enum Mode {
    /// Squash all layers into a single output
    ///
    /// This results in the output directory containing the same equivalent file system
    /// as if the container was actually booted.
    #[default]
    Squash,
}

#[tracing::instrument]
pub async fn main(opts: Options) -> Result<()> {
    info!("Extracting image");

    let output = canonicalize_output_dir(&opts.output_dir, opts.overwrite)?;
    let registry = Registry::builder()
        .maybe_platform(opts.platform)
        .reference(opts.image)
        .build()
        .await
        .context("configure remote registry")?;

    let layers = registry.layers().await.context("list layers")?;
    let count = layers.len();
    info!("enumerated {count} {}", plural(count, "layer", "layers"));

    for (descriptor, layer) in layers.into_iter().zip(1usize..) {
        info!(layer = %descriptor, "applying layer {layer} of {count}");
        registry
            .apply_layer(&descriptor, &output)
            .await
            .with_context(|| format!("apply layer {descriptor} to {output:?}"))?;
    }

    Ok(())
}

/// Given a (probably relative) path to a directory, canonicalize it to an absolute path.
/// If the path already exists, behavior depends on the `overwrite` flag:
/// - If `overwrite` is true, the existing directory is removed and a new one is created.
/// - If `overwrite` is false, an error is returned.
fn canonicalize_output_dir(path: &str, overwrite: bool) -> Result<PathBuf> {
    let path = PathBuf::from(path);

    // If we're able to canonicalize the path, it already exists.
    // We want to remove its contents and recreate it if `overwrite` is true.
    if let Ok(path) = std::fs::canonicalize(&path) {
        if !overwrite {
            bail!("output directory already exists: {path:?}");
        }

        info!(?path, "removing existing output directory");
        std::fs::remove_dir_all(&path).context("remove existing output directory")?;
        std::fs::create_dir(&path).context("create new directory")?;
        return Ok(path);
    }

    // Failed to canonicalize the path, which means it doesn't exist.
    // We need to create it, then canonicalize it now that it exists.
    info!(?path, "creating new output directory");
    std::fs::create_dir_all(&path).context("create parent dir")?;
    std::fs::canonicalize(&path).context("canonicalize path")
}

fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}
