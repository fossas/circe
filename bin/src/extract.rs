use circe_lib::{registry::Registry, Authentication, LayerDescriptor, Platform, Reference};
use clap::{Parser, ValueEnum};
use color_eyre::eyre::{bail, Context, Result};
use derive_more::Debug;
use std::{path::PathBuf, str::FromStr};
use tracing::{debug, info};

#[derive(Debug, Parser)]
pub struct Options {
    /// Image reference being extracted (e.g. docker.io/library/ubuntu:latest)
    #[arg(value_parser = Reference::from_str)]
    image: Reference,

    /// Directory to which the extracted contents will be written
    #[arg(default_value = ".")]
    output_dir: String,

    /// Overwrite the existing output directory if it exists
    #[arg(long, short)]
    overwrite: bool,

    /// Platform to extract (e.g. linux/amd64)
    ///
    /// If the image is not multi-platform, this is ignored.
    /// If the image is multi-platform, this is used to select the platform to extract.
    ///
    /// If the image is multi-platform and this argument is not provided,
    /// the platform is chosen according to the following priority list:
    /// 1. The first platform-independent image
    /// 2. The current platform (if available)
    /// 3. The `linux` platform for the current architecture
    /// 4. The `linux` platform for the `amd64` architecture
    /// 5. The first platform in the image manifest
    #[arg(long, value_parser = Platform::from_str, verbatim_doc_comment)]
    platform: Option<Platform>,

    /// How to handle layers during extraction
    #[arg(long, default_value = "squash")]
    layers: Mode,

    /// The username to use for authenticating to the registry
    #[arg(long, requires = "password")]
    username: Option<String>,

    /// The password to use for authenticating to the registry
    #[arg(long, requires = "username")]
    #[debug(skip)]
    password: Option<String>,
}

#[derive(Copy, Clone, Debug, Default, ValueEnum)]
pub enum Mode {
    /// Squash all layers into a single output
    ///
    /// This results in the output directory containing the same equivalent file system
    /// as if the container was actually booted.
    #[default]
    Squash,

    /// Only extract the base layer.
    Base,

    /// Extract all layers to a separate directory for each layer.
    /// Also writes a `layers.json` file containing the list of layers in application order.
    Separate,
}

#[tracing::instrument]
pub async fn main(opts: Options) -> Result<()> {
    info!("extracting image");

    let auth = match (opts.username, opts.password) {
        (Some(username), Some(password)) => Authentication::basic(username, password),
        _ => Authentication::default(),
    };

    let output = canonicalize_output_dir(&opts.output_dir, opts.overwrite)?;
    let registry = Registry::builder()
        .maybe_platform(opts.platform)
        .reference(opts.image)
        .auth(auth)
        .build()
        .await
        .context("configure remote registry")?;

    let layers = registry.layers().await.context("list layers")?;
    match opts.layers {
        Mode::Squash => squash(&registry, &output, layers).await,
        Mode::Base => squash(&registry, &output, layers.into_iter().take(1)).await,
        Mode::Separate => separate(&registry, &output, layers).await,
    }
}

async fn squash(
    registry: &Registry,
    output: &PathBuf,
    layers: impl IntoIterator<Item = LayerDescriptor>,
) -> Result<()> {
    let layers = layers.into_iter();
    let count = layers.size_hint();
    let count = count.1.unwrap_or(count.0);
    info!("enumerated {count} {}", plural(count, "layer", "layers"));

    for (descriptor, layer) in layers.zip(1usize..) {
        debug!(?descriptor, layer, count, "applying layer");
        if count > 0 {
            info!(layer = %descriptor, "applying layer {layer} of {count}");
        } else {
            info!(layer = %descriptor, "applying layer {layer}");
        }

        registry
            .apply_layer(&descriptor, &output)
            .await
            .with_context(|| format!("apply layer {descriptor} to {output:?}"))?;
    }

    info!("finished applying layers");
    Ok(())
}

async fn separate(
    registry: &Registry,
    output: &PathBuf,
    layers: impl IntoIterator<Item = LayerDescriptor>,
) -> Result<()> {
    let layers = layers.into_iter().collect::<Vec<_>>();
    let count = layers.len();
    info!("enumerated {count} {}", plural(count, "layer", "layers"));

    for (descriptor, layer) in layers.iter().zip(1usize..) {
        debug!(?descriptor, layer, count, "applying layer");
        let output = output.join(descriptor.digest.as_hex());
        if count > 0 {
            info!(layer = %descriptor, "applying layer {layer} of {count}");
        } else {
            info!(layer = %descriptor, "applying layer {layer}");
        }

        registry
            .apply_layer(&descriptor, &output)
            .await
            .with_context(|| format!("apply layer {descriptor} to {output:?}"))?;
    }

    info!("finished applying layers");
    let index_destination = output.join("layers.json");
    let index = layers
        .into_iter()
        .map(|l| l.digest.as_hex())
        .collect::<Vec<_>>();

    debug!(?index, ?index_destination, "serializing layer index");
    let index = serde_json::to_string_pretty(&index).context("serialize layer index")?;
    tokio::fs::write(&index_destination, index)
        .await
        .context("write layer index")
        .inspect(|_| info!(path = ?index_destination, "layer index written"))
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
