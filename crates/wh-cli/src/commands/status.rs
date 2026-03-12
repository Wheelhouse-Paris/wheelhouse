//! `wh status` command implementation (AC#2).
//!
//! Connects to the broker's control socket and requests health data.
//! Supports `--format json` for machine-readable output.
//! Human-readable output uses approved vocabulary -- never "broker" (RT-B1).

use crate::client::ControlClient;
use crate::output::{self, OutputFormat};

/// Execute the `wh status` command.
pub async fn execute(format: OutputFormat) {
    let client = ControlClient::new();

    match client.send_command("status").await {
        Ok(response) => {
            let status = response
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("unknown");

            if status == "ok" {
                output::print_status(&response, format);
            } else {
                output::print_error(&response, format);
                std::process::exit(1);
            }
        }
        Err(e) => {
            e.exit();
        }
    }
}
