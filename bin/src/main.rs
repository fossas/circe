use clap::{
    builder::{styling::AnsiColor, Styles},
    Parser,
};
use color_eyre::eyre::Result;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{self, prelude::*};

mod extract;
mod list;
mod reexport;

#[derive(Debug, Parser)]
#[command(version, about, styles = style())]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Parser)]
enum Commands {
    /// Extract OCI image to a directory
    Extract(extract::Options),

    /// Enumerate the layers and files in an OCI image
    List(list::Options),

    /// Re-export an OCI image for FOSSA CLI
    ///
    /// Unless you're employed by FOSSA, this is almost definitely not what you want.
    ///
    /// Circe is intended to support FOSSA CLI in its ability to pull images from remote OCI hosts that use a different
    /// container format than the one FOSSA CLI is built to support.
    ///
    /// Meanwhile FOSSA CLI has been built with the assumption that the tarball is the baseline unit
    /// of container scanning; all operations end with "... and then make it a tarball and scan it".
    /// Untangling this and turning it into "scan the contents of a directory" is a larger lift
    /// than this project currently has budget for.
    ///
    /// As such, this subcommand causes Circe to work around this by becoming a "middle layer":
    /// it pulls the image and re-bundles it into a tar format FOSSA CLI knows how to support.
    ///
    /// Note that this image is not meant to be generally useful: there are no
    /// guarantees about the format of the image with respect to Docker or any other tool.
    ///
    /// In the future we will likely refactor FOSSA CLI to be able to work with the data extracted
    /// by Circe directly, at which point this command may be removed entirely and at minimum will
    /// receive no further changes.
    #[clap(verbatim_doc_comment)]
    Reexport(reexport::Options),
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::registry()
        .with(tracing_error::ErrorLayer::default())
        .with(
            tracing_tree::HierarchicalLayer::default()
                .with_indent_lines(true)
                .with_indent_amount(2)
                .with_thread_ids(false)
                .with_thread_names(false)
                .with_verbose_exit(false)
                .with_verbose_entry(false)
                .with_deferred_spans(true)
                .with_bracketed_fields(true)
                .with_span_retrace(true)
                .with_targets(false),
        )
        .with(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Extract(opts) => extract::main(opts).await?,
        Commands::List(opts) => list::main(opts).await?,
        Commands::Reexport(opts) => reexport::main(opts).await?,
    }

    Ok(())
}

fn style() -> Styles {
    Styles::styled()
        .header(AnsiColor::Yellow.on_default())
        .usage(AnsiColor::Green.on_default())
        .literal(AnsiColor::Green.on_default())
        .placeholder(AnsiColor::Green.on_default())
        .error(AnsiColor::Red.on_default())
        .invalid(AnsiColor::Red.on_default())
        .valid(AnsiColor::Blue.on_default())
}
