use clap::{Parser, Subcommand};
use std::process;

use wh_cli::commands::deploy::DeployCommand;

/// wh - Wheelhouse CLI
///
/// Manage topology, streams, and agents.
#[derive(Debug, Parser)]
#[command(name = "wh", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Manage topology deployment
    Deploy {
        #[command(subcommand)]
        command: DeployCommand,
    },
}

fn main() {
    let cli = Cli::parse();

    let exit_code = match cli.command {
        Commands::Deploy { command } => command.execute(),
    };

    process::exit(exit_code);
}
