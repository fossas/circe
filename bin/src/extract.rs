use circe_lib::{
    docker::{Daemon, Tarball},
    extract::{extract, Report, Strategy},
    registry::Registry,
    Authentication, Filters, Platform, Reference, Source,
};
use clap::{Args, Parser, ValueEnum};
use color_eyre::eyre::{bail, Context, Result};
use derive_more::Debug;
use std::{path::PathBuf, str::FromStr};
use tracing::{debug, info};

use crate::try_strategies;

#[derive(Debug, Parser)]
pub struct Options {
    /// Target to extract
    #[clap(flatten)]
    target: Target,

    /// Directory to which the extracted contents will be written
    ///
    /// Layers are extracted into subdirectories based on the `layers` option.
    /// An `image.json` file is written to this directory with details about the extracted content.
    #[arg(default_value = ".")]
    output_dir: String,

    /// Overwrite the existing output directory if it exists
    #[arg(long, short)]
    overwrite: bool,

    /// How to handle layers during extraction
    #[arg(long, default_value = "squash")]
    layers: Mode,

    /// Glob filters for layers to extract
    ///
    /// Filters are unix-style glob patterns, for example `sha256:1234*`
    /// matches any layer with a sha256 digest starting with `1234`.
    ///
    /// You can provide this multiple times to provide multiple filters.
    /// If filters are provided, only layers whose digest matches any filter are extracted.
    #[arg(long, alias = "lg")]
    layer_glob: Option<Vec<String>>,

    /// Glob filters for files to extract
    ///
    /// Filters are unix-style glob patterns, for example `*.txt`
    /// matches any file whose path ends with `.txt`.
    /// Note that if you want to match regardless of directory depth
    /// you must use `**` in the pattern, for example `**/*.txt` matches
    /// any file with a `.txt` extension in any directory.
    ///
    /// Non-unicode paths are lossily parsed as unicode for the purpose of glob comparison;
    /// invalid unicode segments are replaced with `U+FFFD` (�).
    ///
    /// You can provide this multiple times to provide multiple filters.
    /// If filters are provided, only files whose path matches any filter are extracted.
    #[arg(long, alias = "fg")]
    file_glob: Option<Vec<String>>,

    /// Regex filters for layers to extract
    ///
    /// Filters are regex patterns, for example `sha256:1234.*`
    /// matches any layer with a sha256 digest starting with `1234`.
    ///
    /// You can provide this multiple times to provide multiple filters.
    /// If filters are provided, only layers whose digest matches any filter are extracted.
    #[arg(long, alias = "lr")]
    layer_regex: Option<Vec<String>>,

    /// Regex filters for files to extract
    ///
    /// Filters are regex patterns, for example `.*\.txt$`
    /// matches any file whose path ends with `.txt`.
    ///
    /// Non-unicode paths are lossily parsed as unicode for the purpose of regex comparison;
    /// invalid unicode segments are replaced with `U+FFFD` ().
    ///
    /// You can provide this multiple times to provide multiple filters.
    /// If filters are provided, only files whose path matches any filter are extracted.
    #[arg(long, alias = "fr")]
    file_regex: Option<Vec<String>>,
}

/// Shared options for any command that needs to work with the OCI registry for a given image.
#[derive(Debug, Args)]
pub struct Target {
    /// Image reference being extracted (e.g. docker.io/library/ubuntu:latest)
    ///
    /// If a fully specified reference is not provided,
    /// the image is attempted to be resolved with the prefix
    /// `docker.io/library`.
    ///
    /// The reference may optionally provide a digest, for example
    /// `docker.io/library/ubuntu@sha256:1234567890`.
    ///
    /// Finally, the reference may optionally provide a tag, for example
    /// `docker.io/library/ubuntu:latest` or `docker.io/library/ubuntu:24.04`.
    /// If no digest or tag is provided, the tag "latest" is used.
    ///
    /// Put all that together and you get the following examples:
    /// - `ubuntu` is resolved as `docker.io/library/ubuntu:latest`
    /// - `ubuntu:24.04` is resolved as `docker.io/library/ubuntu:24.04`
    /// - `docker.io/library/ubuntu` is resolved as `docker.io/library/ubuntu:latest`
    /// - `docker.io/library/ubuntu@sha256:1234567890` is resolved as `docker.io/library/ubuntu@sha256:1234567890`
    /// - `docker.io/library/ubuntu:24.04` is resolved as `docker.io/library/ubuntu:24.04`
    #[arg(verbatim_doc_comment)]
    pub image: String,

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
    pub platform: Option<Platform>,

    /// The username to use for authenticating to the registry
    #[arg(long, requires = "password")]
    pub username: Option<String>,

    /// The password to use for authenticating to the registry
    #[arg(long, requires = "username")]
    #[debug(skip)]
    pub password: Option<String>,
}

#[derive(Copy, Clone, Debug, Default, ValueEnum)]
pub enum Mode {
    /// Squash all layers into a single output directory, resulting in a file system equivalent to a running container.
    #[default]
    Squash,

    /// Only extract the base layer.
    Base,

    /// Squash all "other" layers; "other" layers are all layers except the base layer.
    SquashOther,

    /// Extract the base layer and all "other" layers; "other" layers are all layers except the base layer.
    BaseAndSquashOther,

    /// Extract all layers to a separate directory for each layer, with each directory named after the layer's digest.
    Separate,
}

#[tracing::instrument]
pub async fn main(opts: Options) -> Result<()> {
    info!("extracting image");
    try_strategies!(&opts; strategy_registry, strategy_daemon, strategy_tarball)
}

async fn strategy_registry(opts: &Options) -> Result<()> {
    let reference = Reference::from_str(&opts.target.image)?;
    let layer_globs = Filters::parse_glob(opts.layer_glob.iter().flatten())?;
    let file_globs = Filters::parse_glob(opts.file_glob.iter().flatten())?;
    let layer_regexes = Filters::parse_regex(opts.layer_regex.iter().flatten())?;
    let file_regexes = Filters::parse_regex(opts.file_regex.iter().flatten())?;
    let auth = match (&opts.target.username, &opts.target.password) {
        (Some(username), Some(password)) => Authentication::basic(username, password),
        _ => Authentication::docker(&reference).await?,
    };

    let registry = Registry::builder()
        .maybe_platform(opts.target.platform.as_ref())
        .reference(reference)
        .auth(auth)
        .layer_filters(layer_globs + layer_regexes)
        .file_filters(file_globs + file_regexes)
        .build()
        .await
        .context("configure remote registry")?;

    extract_layers(opts, registry).await.context("list files")
}

async fn strategy_daemon(opts: &Options) -> Result<()> {
    let layer_globs = Filters::parse_glob(opts.layer_glob.iter().flatten())?;
    let file_globs = Filters::parse_glob(opts.file_glob.iter().flatten())?;
    let layer_regexes = Filters::parse_regex(opts.layer_regex.iter().flatten())?;
    let file_regexes = Filters::parse_regex(opts.file_regex.iter().flatten())?;

    let daemon = Daemon::builder()
        .reference(&opts.target.image)
        .layer_filters(layer_globs + layer_regexes)
        .file_filters(file_globs + file_regexes)
        .build()
        .await
        .context("build daemon reference")?;

    tracing::info!("pulled image from daemon");
    extract_layers(opts, daemon).await.context("list files")
}

async fn strategy_tarball(opts: &Options) -> Result<()> {
    let path = PathBuf::from(&opts.target.image);
    if matches!(tokio::fs::try_exists(&path).await, Err(_) | Ok(false)) {
        bail!("path does not exist: {path:?}");
    }

    let layer_globs = Filters::parse_glob(opts.layer_glob.iter().flatten())?;
    let file_globs = Filters::parse_glob(opts.file_glob.iter().flatten())?;
    let layer_regexes = Filters::parse_regex(opts.layer_regex.iter().flatten())?;
    let file_regexes = Filters::parse_regex(opts.file_regex.iter().flatten())?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| opts.target.image.clone().into())
        .to_string();

    let tarball = Tarball::builder()
        .path(path)
        .name(name)
        .file_filters(file_globs + file_regexes)
        .layer_filters(layer_globs + layer_regexes)
        .build()
        .await
        .context("build tarball reference")?;

    tracing::info!("pulled image from daemon");
    extract_layers(opts, tarball).await.context("list files")
}

#[tracing::instrument]
async fn extract_layers(opts: &Options, registry: impl Source) -> Result<()> {
    let layers = registry.layers().await.context("list layers")?;
    if layers.is_empty() {
        bail!("no layers to extract found in image");
    }

    let strategies = match opts.layers {
        Mode::Squash => vec![Strategy::Squash(layers)],
        Mode::SquashOther => vec![Strategy::Squash(layers.into_iter().skip(1).collect())],
        Mode::Base => vec![Strategy::Squash(layers.into_iter().take(1).collect())],
        Mode::Separate => layers.into_iter().map(Strategy::Separate).collect(),
        Mode::BaseAndSquashOther => match layers.as_slice() {
            [] => unreachable!(),
            [base] => vec![Strategy::Separate(base.clone())],
            [base, rest @ ..] => vec![
                Strategy::Separate(base.clone()),
                Strategy::Squash(rest.to_vec()),
            ],
        },
    };

    let output = canonicalize_output_dir(&opts.output_dir, opts.overwrite)?;
    let digest = registry.digest().await.context("fetch digest")?;
    let layers = extract(&registry, &output, strategies)
        .await
        .context("extract image")?;

    let report = Report::builder()
        .digest(digest.to_string())
        .layers(layers)
        .build();

    report
        .write(&output)
        .await
        .context("write report to disk")?;

    println!("{}", report.render()?);

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
