use circe::{Platform, Reference};
use clap::{Parser, ValueEnum};
use color_eyre::eyre::{Context, Result};
use std::{path::PathBuf, str::FromStr};
use tracing::info;

#[derive(Debug, Parser)]
pub struct Options {
    /// Image reference being extracted (e.g. docker.io/library/ubuntu:latest)
    #[arg(value_parser = Reference::from_str)]
    image: Reference,

    /// Directory to which the extracted contents will be written
    #[arg(default_value = ".", value_parser = canonicalize)]
    output_dir: PathBuf,

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

    Ok(())
}

fn canonicalize(path: &str) -> Result<PathBuf> {
    std::fs::canonicalize(path).context("canonicalize path")
}
