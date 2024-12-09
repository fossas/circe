use clap::Parser;
use color_eyre::eyre::Result;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{self, prelude::*};

mod cmd;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Parser)]
enum Commands {
    /// Extract OCI image to a directory
    Extract(cmd::extract::Args),
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
                .with_thread_names(true)
                .with_thread_ids(true)
                .with_verbose_exit(false)
                .with_verbose_entry(false)
                .with_targets(true),
        )
        .with(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .init();

    let cli = Cli::try_parse()?;
    match cli.command {
        Commands::Extract(args) => cmd::extract::main(args).await?,
    }

    Ok(())
}
