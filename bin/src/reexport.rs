use async_tempfile::TempFile;
use circe_lib::{
    docker::{Daemon, Tarball},
    fossacli::{Image, Manifest, ManifestEntry, RootFs},
    registry::Registry,
    Authentication, Digest, Reference, Source,
};
use clap::Parser;
use color_eyre::eyre::{bail, Context, Result};
use derive_more::Debug;
use pluralizer::pluralize;
use std::{path::PathBuf, str::FromStr};
use tap::Pipe;
use tokio_tar::Builder;
use tracing::{debug, info, warn};

use crate::{extract::Target, try_strategies, Outcome};

#[derive(Debug, Parser)]
pub struct Options {
    /// Target container image to re-export
    #[clap(flatten)]
    target: Target,

    /// File path where the re-exported tarball will be written
    #[arg(default_value = "image.tar")]
    output: String,
}

#[tracing::instrument]
pub async fn main(opts: Options) -> Result<()> {
    info!("re-exporting image for FOSSA CLI");
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

    let tag = format!("{}:{}", reference.name, reference.version);
    let registry = Registry::builder()
        .maybe_platform(opts.target.platform.as_ref())
        .reference(reference.clone())
        .auth(auth)
        .build()
        .await
        .context("configure remote registry")?;

    reexport(opts, tag, registry)
        .await
        .context("reexporting image")
        .map(|_| Outcome::Success)
}

async fn strategy_daemon(opts: &Options) -> Result<Outcome> {
    if opts.target.is_path().await {
        debug!("input appears to be a file path, skipping strategy");
        return Ok(Outcome::Skipped);
    }

    let tag = opts.target.image.clone();
    let daemon = Daemon::builder()
        .reference(&tag)
        .build()
        .await
        .context("build daemon reference")?;

    tracing::info!("pulled image from daemon");
    reexport(opts, tag, daemon)
        .await
        .context("reexporting image")
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

    tracing::info!(path = %path.display(), name = %name, "using local tarball");
    let tarball = Tarball::builder()
        .path(path)
        .name(&name)
        .build()
        .await
        .context("build tarball reference")?;

    let digest = tarball.digest().await.context("get image digest")?.as_hex();
    let tag = format!("{name}:{digest}");

    tracing::info!(tag = %tag, "created tag for reexport");
    reexport(opts, tag, tarball)
        .await
        .context("reexporting image")
        .map(|_| Outcome::Success)
}

#[tracing::instrument]
async fn reexport(opts: &Options, tag: String, registry: impl Source) -> Result<()> {
    let layers = registry.layers().await.context("list layers")?;
    let count = layers.len();
    info!("enumerated {}", pluralize("layer", count as isize, true));

    // FOSSA CLI container scans start here:
    // https://github.com/fossas/fossa-cli/blob/85a6977cb13ec2b8c5486dbbe464c61d6608bbd3/src/App/Fossa/Container/Scan.hs#L90
    //
    // It picks a strategy (Docker Archive, Docker Engine, Podman, or direct download from registry)
    // and in all cases it first downloads a tarball representing the image and then parses it.
    // For example, "direct download" is just "pull remote and export it to tarball" and then
    // "open the tarball for parsing": https://github.com/fossas/fossa-cli/blob/85a6977cb13ec2b8c5486dbbe464c61d6608bbd3/src/App/Fossa/Container/Sources/Registry.hs#L84
    //
    // The main function that all these branches call is `analyzeFromDockerArchive`, which is here: https://github.com/fossas/fossa-cli/blob/3a003190692b66780d76210ee0fb35ac6375c8d2/src/App/Fossa/Container/Sources/DockerArchive.hs#L104
    // This first parses the image into a `ContainerImageRaw` type: https://github.com/fossas/fossa-cli/blob/65046d8b1935a2693e6f30869afbc2efb868352e/src/Container/Tarball.hs#L61-L62
    // This type is made up of two parts:
    // - The `ManifestJson` (corresponds to `circe_lib::fossacli::Manifest`)
    // - A collection of `ContainerLayer`s
    //
    // The `ManifestJson` type is parsed by walking the outermost layer of the tarball and attempting to parse every file entry,
    // however the `millhone` CLI requires that it is named `manifest.json`:
    // - https://github.com/fossas/fossa-cli/blob/65046d8b1935a2693e6f30869afbc2efb868352e/src/Container/Tarball.hs#L65-L74
    // - https://github.com/fossas/fossa-cli/blob/e9e8adeaa94c8b225826c25f1e39868c7d38bf79/extlib/millhone/src/cmd/analyze_container.rs#L217-L220
    //
    // After parsing the manifest, FOSSA CLI immediately tries to parse the file at the path indicated by the `config` key:
    // - https://github.com/fossas/fossa-cli/blob/65046d8b1935a2693e6f30869afbc2efb868352e/src/Container/Tarball.hs#L72
    //
    // It then builds a representation of the image based on the combination of these two files:
    // - https://github.com/fossas/fossa-cli/blob/65046d8b1935a2693e6f30869afbc2efb868352e/src/Container/Tarball.hs#L74
    //
    // It's a lot less error prone to use the disk as working state for the tarball we create:
    // the `tokio-tar` library automatically creates a lot of metadata for us if it can use an on-disk artifact
    // which we'd otherwise be stuck recreating.
    //
    // While this comes at the cost of a little more IO (we're indirecting through the disk)
    // I think this is worth the cost unless it demonstrates to the contrary..
    let digest = registry.digest().await.context("get image digest")?;
    let tarball = TempFile::new().await.context("create tarball")?;
    let mut tarball = Builder::new(tarball);
    let mut written = Vec::new();

    for (layer, sequence) in layers.into_iter().zip(1usize..) {
        info!(layer = %layer, %sequence, "reading layer");

        let Some(layer_tarball) = registry
            .layer_plain_tarball(&layer)
            .await
            .context("fetch layer tarball")?
        else {
            warn!(layer = %layer, %sequence, "skipped layer");
            continue;
        };

        tarball
            .append_path_with_name(layer_tarball.file_path(), layer.digest.tarball_filename())
            .await
            .context("add layer to tarball")?;

        info!(layer = %layer, %sequence, filename = %layer.digest.tarball_filename(), "added layer to tarball");
        written.push(layer.digest.clone());
    }

    let (manifest, manifest_content) = ManifestEntry::builder()
        .config(Image::filename(&digest))
        .repo_tags(&tag)
        .layers(written.iter().map(Digest::tarball_filename))
        .build()
        .pipe(Manifest::singleton)
        .write_tempfile()
        .await
        .context("write manifest")?;
    tarball
        .append_path_with_name(manifest.file_path(), Manifest::filename())
        .await
        .context("add manifest to tarball")?;
    info!(filename = %Manifest::filename().display(), manifest = %manifest_content, "added manifest to tarball");

    let (image, image_content) = Image::from(RootFs::layers(written))
        .write_tempfile()
        .await
        .context("write image")?;
    tarball
        .append_path_with_name(image.file_path(), Image::filename(&digest))
        .await
        .context("add image to tarball")?;
    info!(filename = %Image::filename(&digest).display(), image = %image_content, "added image to tarball");

    let tarball = tarball.into_inner().await.context("finish tarball")?;
    tarball.sync_all().await.context("sync tarball")?;
    tokio::fs::copy(tarball.file_path(), &opts.output)
        .await
        .context("copy tarball to destination")?;
    info!(filename = %opts.output, "copied final tarball to destination");

    Ok(())
}
