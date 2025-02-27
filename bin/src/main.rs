use clap::{
    builder::{styling::AnsiColor, Styles},
    Parser,
};
use color_eyre::eyre::Result;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{self, prelude::*};

mod extract;
mod list;
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
