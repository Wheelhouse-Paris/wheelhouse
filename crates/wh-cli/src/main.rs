//! wh — Wheelhouse CLI entry point.
//!
//! Error handling uses `WhError` with manual exit codes.
//! All library modules use typed errors via `thiserror` (SCV-04).

use clap::{Parser, Subcommand};

use wh_cli::commands::deploy::DeployCommand;
use wh_cli::commands::ps::{self, PsArgs};
use wh_cli::commands::secrets::SecretsCmd;
use wh_cli::output::{OutputEnvelope, OutputFormat};

/// wh — the Wheelhouse CLI.
///
/// Unified control plane for operators and agents.
#[derive(Debug, Parser)]
#[command(name = "wh", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// List all deployed components with their live status.
    Ps(PsArgs),
    /// Manage Wheelhouse secrets and credentials.
    Secrets {
        #[command(subcommand)]
        cmd: SecretsCmd,
    },
    /// Manage topology deployment.
    Deploy {
        #[command(subcommand)]
        command: DeployCommand,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Ps(args) => {
            let fmt = args.format;
            let result = ps::execute(&args).map_err(|e| Box::new(e) as Box<dyn std::error::Error>);
            if let Err(e) = result {
                match fmt {
                    OutputFormat::Json => {
                        let envelope = OutputEnvelope::<()>::error("ERROR", e.to_string());
                        if let Ok(json) = serde_json::to_string_pretty(&envelope) {
                            eprintln!("{json}");
                        } else {
                            eprintln!("Error: {e}");
                        }
                    }
                    OutputFormat::Human => {
                        eprintln!("Error: {e}");
                    }
                }
                std::process::exit(1);
            }
        }
        Commands::Secrets { cmd } => {
            let fmt = cmd.format();
            let result = cmd.run().map_err(|e| Box::new(e) as Box<dyn std::error::Error>);
            if let Err(e) = result {
                match fmt {
                    OutputFormat::Json => {
                        let envelope = OutputEnvelope::<()>::error("ERROR", e.to_string());
                        if let Ok(json) = serde_json::to_string_pretty(&envelope) {
                            eprintln!("{json}");
                        } else {
                            eprintln!("Error: {e}");
                        }
                    }
                    OutputFormat::Human => {
                        eprintln!("Error: {e}");
                    }
                }
                std::process::exit(1);
            }
        }
        Commands::Deploy { command } => {
            let exit_code = command.execute();
            std::process::exit(exit_code);
        }
    }
}
