//! wh — Wheelhouse CLI entry point.
//!
//! Routes subcommands to their implementations. `anyhow` is permitted here
//! per architecture rules (SCV-04).

use clap::{Parser, Subcommand};
use wh_cli::commands::surface::{self, SurfaceCommand};

/// Wheelhouse CLI — operating infrastructure for autonomous agent factories.
#[derive(Debug, Parser)]
#[command(name = "wh", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Top-level CLI commands.
#[derive(Debug, Subcommand)]
enum Commands {
    /// Manage surfaces (CLI, Telegram, etc.).
    Surface {
        #[command(subcommand)]
        command: SurfaceCommand,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Surface { command } => match command {
            SurfaceCommand::Cli { stream, format } => {
                if let Err(e) = surface::run_cli(&stream, &format).await {
                    e.exit();
                }
            }
        },
    }

    Ok(())
}
