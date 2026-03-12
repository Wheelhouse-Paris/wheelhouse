//! wh — Wheelhouse CLI entry point.
//!
//! Error handling uses `WhError` with manual exit codes.
//! All library modules use typed errors via `thiserror` (SCV-04).

use clap::Parser;

use wh_cli::commands::completion;
use wh_cli::commands::logs;
use wh_cli::commands::ps;
use wh_cli::commands::status;
use wh_cli::commands::stream;
use wh_cli::commands::surface::{self, SurfaceCommand};
use wh_cli::output::{OutputEnvelope, OutputFormat};
use wh_cli::{Cli, Commands};

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
        Commands::Memory { command } => {
            let exit_code = command.execute();
            std::process::exit(exit_code);
        }
        Commands::Compact(args) => {
            let exit_code = args.execute().await;
            std::process::exit(exit_code);
        }
        Commands::Stream { command } => {
            stream::execute(&command).await;
        }
        Commands::Status { format } => {
            status::execute(format).await;
        }
        Commands::Completion(args) => {
            completion::execute(&args);
        }
        Commands::Doctor(args) => {
            let exit_code = args.execute();
            std::process::exit(exit_code);
        }
    }
}
