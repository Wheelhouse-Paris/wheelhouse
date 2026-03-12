//! wh — Wheelhouse CLI entry point.
//!
//! Error handling uses `WhError` with manual exit codes.
//! All library modules use typed errors via `thiserror` (SCV-04).

use clap::{Parser, Subcommand};

use wh_cli::commands::ps::{self, PsArgs};
use wh_cli::output::json;
use wh_cli::output::OutputFormat;

/// Wheelhouse CLI — manage your agent topology.
#[derive(Parser)]
#[command(name = "wh", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List all deployed components with their live status.
    Ps(PsArgs),
}

fn main() {
    let cli = Cli::parse();

    let result = match &cli.command {
        Commands::Ps(args) => ps::execute(args).map_err(|e| (e, args.format)),
    };

    if let Err((err, format)) = result {
        let exit_code = err.exit_code();
        match format {
            OutputFormat::Json => {
                json::print_json_error(&err);
            }
            OutputFormat::Human => {
                eprintln!("Error: {err}");
            }
        }
        std::process::exit(exit_code);
    }
}
