use clap::{Parser, Subcommand};
use wh_cli::commands::deploy::{self, DeployCmd};
use wh_cli::output::Format;

/// wh — Wheelhouse CLI
#[derive(Parser)]
#[command(name = "wh", about = "Wheelhouse — agentic infrastructure as code")]
struct Cli {
    /// Output format: human (default) or json.
    #[arg(long, global = true, default_value = "human")]
    format: Format,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage topology deployments.
    #[command(subcommand)]
    Deploy(DeployCmd),
}

fn main() {
    let cli = Cli::parse();

    let exit_code = match cli.command {
        Commands::Deploy(cmd) => match deploy::run(cmd, cli.format) {
            Ok(code) => code,
            Err(e) => {
                match cli.format {
                    Format::Human => {
                        eprintln!("error: {e}");
                    }
                    Format::Json => {
                        let envelope =
                            wh_cli::output::ErrorEnvelope::new(e.error_code(), e.to_string());
                        if let Ok(json) = serde_json::to_string_pretty(&envelope) {
                            println!("{json}");
                        } else {
                            eprintln!("error: {e}");
                        }
                    }
                }
                e.exit_code()
            }
        },
    };

    std::process::exit(exit_code);
}
