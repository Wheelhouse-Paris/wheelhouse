//! wh — Wheelhouse CLI entry point.
//!
//! Error handling uses `WhError` with manual exit codes.
//! All library modules use typed errors via `thiserror` (SCV-04).

use clap::{Parser, Subcommand};

use wh_cli::commands::deploy::DeployCommand;
use wh_cli::commands::logs::{self, LogsArgs};
use wh_cli::commands::ps::{self, PsArgs};
use wh_cli::commands::secrets::SecretsCmd;
use wh_cli::commands::status;
use wh_cli::commands::stream::{self, StreamCommand};
use wh_cli::commands::surface::{self, SurfaceCommand};
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
    /// Tail structured logs from a specific agent.
    Logs(LogsArgs),
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
    /// Manage surfaces (CLI, Telegram, etc.).
    Surface {
        #[command(subcommand)]
        command: SurfaceCommand,
    },
    /// Manage and observe streams.
    Stream {
        #[command(subcommand)]
        command: StreamCommand,
    },
    /// Check Wheelhouse health and status.
    Status {
        /// Output format: human (default) or json.
        #[arg(long, default_value = "human")]
        format: OutputFormat,
    },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Ps(args) => {
            let fmt = args.format;
            let result = ps::execute(&args).await.map_err(|e| Box::new(e) as Box<dyn std::error::Error>);
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
        Commands::Logs(args) => {
            let fmt = args.format;
            let result = logs::execute(&args);
            if let Err(ref err) = result {
                match fmt {
                    OutputFormat::Json => {
                        let envelope = OutputEnvelope::<()>::error(err.error_code(), err.to_string());
                        if let Ok(json) = serde_json::to_string_pretty(&envelope) {
                            eprintln!("{json}");
                        } else {
                            eprintln!("Error: {err}");
                        }
                    }
                    OutputFormat::Human => {
                        eprintln!("Error: {err}");
                    }
                }
                std::process::exit(err.exit_code());
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
        Commands::Surface { command } => {
            match command {
                SurfaceCommand::Cli { stream, format } => {
                    if let Err(e) = surface::run_cli(&stream, &format).await {
                        eprintln!("{e}");
                        std::process::exit(1);
                    }
                }
            }
        }
        Commands::Stream { command } => {
            stream::execute(&command).await;
        }
        Commands::Status { format } => {
            status::execute(format).await;
        }
    }
}
