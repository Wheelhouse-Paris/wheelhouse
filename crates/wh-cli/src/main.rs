use clap::{Parser, Subcommand};

use wh_cli::commands::secrets::SecretsCmd;
use wh_cli::output::Format;

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
    /// Manage Wheelhouse secrets and credentials.
    Secrets {
        #[command(subcommand)]
        cmd: SecretsCmd,
    },
}

fn main() {
    let cli = Cli::parse();

    let (result, format) = match cli.command {
        Commands::Secrets { cmd } => {
            let fmt = cmd.format();
            (cmd.run(), fmt)
        }
    };

    if let Err(e) = result {
        let code = e.exit_code();
        match format {
            Format::Json => {
                let envelope = wh_cli::output::OutputEnvelope::<()>::error(
                    e.error_code(),
                    e.to_string(),
                );
                if let Ok(json) = serde_json::to_string_pretty(&envelope) {
                    eprintln!("{json}");
                } else {
                    eprintln!("Error: {e}");
                }
            }
            Format::Human => {
                eprintln!("Error: {e}");
            }
        }
        std::process::exit(code);
    }
}
