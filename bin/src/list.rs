use circe_lib::{
    docker::{Daemon, Tarball},
    registry::Registry,
    Authentication, Reference, Source,
};
use clap::Parser;
use color_eyre::eyre::{bail, Context, Result};
use derive_more::Debug;
use pluralizer::pluralize;
use std::{collections::HashMap, path::PathBuf, str::FromStr};
use tracing::{debug, info};

use crate::{extract::Target, try_strategies, Outcome};

#[derive(Debug, Parser)]
pub struct Options {
    /// Target container image to list layers and files from
    #[clap(flatten)]
    target: Target,
}

#[tracing::instrument]
pub async fn main(opts: Options) -> Result<()> {
    info!("extracting image");
    try_strategies!(&opts; strategy_tarball, strategy_daemon, strategy_registry)
}

async fn strategy_registry(opts: &Options) -> Result<Outcome> {
    if opts.target.is_path().await {
        debug!("input appears to be a file path, skipping strategy");
        return Ok(Outcome::Skipped);
    }

    let reference = Reference::from_str(&opts.target.image)?;
    let auth = match (&opts.target.username, &opts.target.password) {
        (Some(username), Some(password)) => Authentication::basic(username, password),
        _ => Authentication::docker(&reference).await?,
    };

    let registry = Registry::builder()
        .maybe_platform(opts.target.platform.as_ref())
        .reference(reference)
        .auth(auth)
        .build()
        .await
        .context("configure remote registry")?;

    list_files(registry)
        .await
        .context("list files")
        .map(|_| Outcome::Success)
}

async fn strategy_daemon(opts: &Options) -> Result<Outcome> {
    if opts.target.is_path().await {
        debug!("input appears to be a file path, skipping strategy");
        return Ok(Outcome::Skipped);
    }

    let daemon = Daemon::builder()
        .reference(&opts.target.image)
        .build()
        .await
        .context("build daemon reference")?;

    tracing::info!("pulled image from daemon");
    list_files(daemon)
        .await
        .context("list files")
        .map(|_| Outcome::Success)
}

async fn strategy_tarball(opts: &Options) -> Result<Outcome> {
    let path = PathBuf::from(&opts.target.image);
    if matches!(tokio::fs::try_exists(&path).await, Err(_) | Ok(false)) {
        bail!("path does not exist: {path:?}");
    }

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| opts.target.image.clone().into())
        .to_string();
    let tarball = Tarball::builder()
        .path(path)
        .name(name)
        .build()
        .await
        .context("build tarball reference")?;

    tracing::info!("listing files in tarball");
    list_files(tarball)
        .await
        .context("list files")
        .map(|_| Outcome::Success)
}

#[tracing::instrument]
async fn list_files(registry: impl Source) -> Result<()> {
    let layers = registry.layers().await.context("list layers")?;
    let count = layers.len();
    debug!(?count, ?layers, "listed layers");
    info!("enumerated {}", pluralize("layer", count as isize, true));

    let mut listing = HashMap::new();
    for (descriptor, layer) in layers.into_iter().zip(1usize..) {
        info!(layer = %descriptor, %layer, "reading layer");
        let files = registry
            .list_files(&descriptor)
            .await
            .context("list files")?;

        debug!(layer = %descriptor, files = %files.len(), "listed files");
        listing.insert(descriptor.digest.to_string(), files);
    }

    let rendered = serde_json::to_string_pretty(&listing).context("render listing")?;
    println!("{rendered}");

    Ok(())
}
