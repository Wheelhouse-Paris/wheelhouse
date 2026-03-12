//! Wheelhouse CLI binary entry point.
//!
//! Uses `clap` derive macros for argument parsing.
//! This is the only file in the CLI crate that may use `anyhow` (SCV-04).

use clap::{Parser, Subcommand};
use wh_cli::output::OutputFormat;

#[derive(Parser)]
#[command(name = "wh", about = "Wheelhouse — agent infrastructure CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show Wheelhouse status and health information.
    Status {
        /// Output format: human or json.
        #[arg(long, value_enum, default_value = "human")]
        format: OutputFormat,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Status { format } => {
            wh_cli::commands::status::execute(format).await;
        }
    }
}
