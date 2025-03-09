use circe_lib::{registry::Registry, Authentication, Reference};
use clap::Parser;
use color_eyre::eyre::{Context, Result};
use derive_more::Debug;
use pluralizer::pluralize;
use std::{collections::HashMap, str::FromStr};
use tracing::{debug, info};

use crate::extract::Target;

#[derive(Debug, Parser)]
pub struct Options {
    /// Target container image to list layers and files from
    #[clap(flatten)]
    target: Target,
}

#[tracing::instrument]
pub async fn main(opts: Options) -> Result<()> {
    info!("extracting image");

    let reference = Reference::from_str(&opts.target.image)?;
    let auth = match (opts.target.username, opts.target.password) {
        (Some(username), Some(password)) => Authentication::basic(username, password),
        _ => Authentication::docker(&reference).await?,
    };

    let registry = Registry::builder()
        .maybe_platform(opts.target.platform)
        .reference(reference)
        .auth(auth)
        .build()
        .await
        .context("configure remote registry")?;

    let layers = registry.layers().await.context("list layers")?;
    let count = layers.len();
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
