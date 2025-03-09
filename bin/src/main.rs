use clap::{
    builder::{styling::AnsiColor, Styles},
    Parser,
};
use color_eyre::{eyre::Result, Section};
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
    /// Unless you work at FOSSA, this is almost definitely not what you want.
    ///
    /// This command helps FOSSA CLI analyze container images by converting from
    /// remote OCI formats to the tar format FOSSA CLI expects. It acts as a compatibility
    /// layer between different container formats.
    ///
    /// FOSSA CLI currently requires tarballs as input for container scanning.
    /// This command pulls container images and repackages them into a compatible tar
    /// format for analysis.
    ///
    /// Important notes:
    /// - The output is specifically for FOSSA CLI consumption
    /// - The tar format is not compatible with Docker or other container tools
    /// - This command may be removed in the future when FOSSA CLI can work directly
    ///   with extracted container data
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

    match Cli::parse().command {
        Commands::Extract(opts) => extract::main(opts).await,
        Commands::List(opts) => list::main(opts).await,
        Commands::Reexport(opts) => reexport::main(opts).await,
    }
    .with_warning(|| {
        concat!(
            "Authentication errors are sometimes reported when the actual issue ",
            "is that the specified image or tag does not exist. ",
            "This depends on the behavior of the remote container registry.",
        )
    })
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
