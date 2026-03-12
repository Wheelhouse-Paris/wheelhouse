//! Output formatting module (SCV-05).
//!
//! All CLI output routes through this module's format switch.
//! Supports human-readable and `--format json` output.
//! Never serialize JSON directly in command handlers.

pub mod error;

use serde_json::Value;

/// Output format for CLI commands.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    /// Human-readable text (default).
    #[default]
    Human,
    /// Machine-readable JSON.
    Json,
}

/// Format and print a status response.
///
/// Routes through the format switch -- human-readable or JSON (SCV-05).
/// Human-readable output uses approved vocabulary -- never "broker" (RT-B1).
pub fn print_status(response: &Value, format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            // Pass through the raw JSON from the broker
            println!(
                "{}",
                serde_json::to_string_pretty(response).unwrap_or_default()
            );
        }
        OutputFormat::Human => {
            print_status_human(response);
        }
    }
}

/// Print status in human-readable format (RT-B1: approved vocabulary only).
fn print_status_human(response: &Value) {
    let data = match response.get("data") {
        Some(d) => d,
        None => {
            eprintln!("Error: Invalid response from Wheelhouse");
            return;
        }
    };

    let uptime = data
        .get("uptime_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let panic_count = data
        .get("panic_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let subscriber_count = data
        .get("subscriber_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let streams = data
        .get("streams")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    // Format uptime
    let hours = uptime / 3600;
    let minutes = (uptime % 3600) / 60;
    let seconds = uptime % 60;

    println!("Wheelhouse is running");
    println!("  Uptime:      {hours}h {minutes}m {seconds}s");
    println!("  Subscribers: {subscriber_count}");
    println!("  Streams:     {streams}");

    if panic_count > 0 {
        println!("  Panics:      {panic_count} (recovered)");
    }
}

/// Print an error response.
pub fn print_error(response: &Value, format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(response).unwrap_or_default()
            );
        }
        OutputFormat::Human => {
            let message = response
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error");
            eprintln!("Error: {message}");
        }
    }
}
