//! `wh reference` — print the full CLI reference document.
//!
//! Generates a comprehensive CLI reference from the live clap `Command` tree.
//! The reference covers 100% of commands, subcommands, arguments, flags,
//! and includes the semantic exit code table (E12-07, E12-08).
//!
//! Output is markdown optimized for LLM consumption (ADR-032).

use clap::{CommandFactory, Parser};

use crate::output::OutputFormat;

/// Arguments for the `wh reference` command.
#[derive(Debug, Parser)]
pub struct ReferenceArgs {
    /// Output format: human (default) or json.
    #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
    pub format: OutputFormat,
}

/// Generate the full CLI reference as a markdown string.
///
/// Introspects the live clap `Command` tree to guarantee 100% coverage (E12-07).
pub fn generate_cli_reference() -> String {
    let cmd = crate::Cli::command();
    let version = cmd.get_version().unwrap_or("unknown");

    let mut out = String::new();

    // Header
    out.push_str("# wh — CLI Reference\n\n");
    out.push_str(&format!("**Version:** {version}\n\n"));
    out.push_str(
        "This document is auto-generated from the `wh` binary's command structure.\n\
         It covers all commands, subcommands, arguments, flags, and exit codes.\n\n",
    );

    // Table of contents
    out.push_str("## Table of Contents\n\n");
    out.push_str("- [Exit Codes](#exit-codes)\n");
    out.push_str("- [Global Options](#global-options)\n");
    out.push_str("- [Commands](#commands)\n");
    for sub in cmd.get_subcommands() {
        if sub.get_name() == "help" {
            continue;
        }
        let name = sub.get_name();
        let anchor = name.replace(' ', "-");
        out.push_str(&format!("  - [{name}](#{anchor})\n"));
    }
    out.push('\n');

    // Exit codes table (E12-08)
    out.push_str("## Exit Codes\n\n");
    out.push_str("| Code | Meaning |\n");
    out.push_str("|------|---------|\n");
    out.push_str("| 0 | Success — command completed without error |\n");
    out.push_str("| 1 | Error — command failed |\n");
    out.push_str("| 2 | Changes detected — `wh topology plan` found differences between desired and current state |\n");
    out.push('\n');

    // Global options
    out.push_str("## Global Options\n\n");
    out.push_str("| Flag | Description |\n");
    out.push_str("|------|-------------|\n");
    out.push_str("| `-h`, `--help` | Print help information |\n");
    out.push_str("| `-V`, `--version` | Print version information |\n");
    out.push('\n');

    // Commands
    out.push_str("## Commands\n\n");
    for sub in cmd.get_subcommands() {
        if sub.get_name() == "help" {
            continue;
        }
        render_command(&mut out, sub, &format!("wh {}", sub.get_name()), 3);
    }

    out
}

/// Render a single command (and its subcommands recursively) as markdown.
fn render_command(out: &mut String, cmd: &clap::Command, full_path: &str, heading_level: usize) {
    let hashes = "#".repeat(heading_level);
    // Heading
    out.push_str(&format!("{hashes} `{full_path}`\n\n"));

    // Description
    if let Some(about) = cmd.get_about() {
        out.push_str(&format!("{about}\n\n"));
    }
    if let Some(long_about) = cmd.get_long_about() {
        // Only add long_about if it differs from about
        let about_str = cmd.get_about().map(|a| a.to_string()).unwrap_or_default();
        let long_str = long_about.to_string();
        if long_str != about_str {
            out.push_str(&format!("{long_str}\n\n"));
        }
    }

    // Usage
    out.push_str(&format!("**Usage:** `{full_path}"));
    let args: Vec<_> = cmd
        .get_arguments()
        .filter(|a| a.get_id() != "help" && a.get_id() != "version")
        .collect();
    let positionals: Vec<_> = args.iter().filter(|a| a.is_positional()).collect();
    let flags: Vec<_> = args.iter().filter(|a| !a.is_positional()).collect();

    for arg in &positionals {
        let id = arg.get_id().as_str();
        if arg.is_required_set() {
            out.push_str(&format!(" <{id}>"));
        } else {
            out.push_str(&format!(" [{id}]"));
        }
    }
    if !flags.is_empty() {
        out.push_str(" [OPTIONS]");
    }
    let subcommands: Vec<_> = cmd
        .get_subcommands()
        .filter(|s| s.get_name() != "help")
        .collect();
    if !subcommands.is_empty() {
        out.push_str(" <COMMAND>");
    }
    out.push_str("`\n\n");

    // Arguments table
    if !positionals.is_empty() {
        out.push_str("**Arguments:**\n\n");
        out.push_str("| Argument | Description | Required |\n");
        out.push_str("|----------|-------------|----------|\n");
        for arg in &positionals {
            let id = arg.get_id().as_str();
            let help = arg.get_help().map(|h| h.to_string()).unwrap_or_default();
            let required = if arg.is_required_set() { "Yes" } else { "No" };
            out.push_str(&format!("| `{id}` | {help} | {required} |\n"));
        }
        out.push('\n');
    }

    // Flags table
    if !flags.is_empty() {
        out.push_str("**Options:**\n\n");
        out.push_str("| Flag | Description | Default |\n");
        out.push_str("|------|-------------|----------|\n");
        for arg in &flags {
            let long = arg.get_long().map(|l| format!("--{l}")).unwrap_or_default();
            let short = arg
                .get_short()
                .map(|s| format!("-{s}, "))
                .unwrap_or_default();
            let flag_str = format!("{short}{long}");
            let help = arg.get_help().map(|h| h.to_string()).unwrap_or_default();
            let default = arg
                .get_default_values()
                .iter()
                .map(|v| v.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let default_str = if default.is_empty() {
                "—".to_string()
            } else {
                format!("`{default}`")
            };
            out.push_str(&format!("| `{flag_str}` | {help} | {default_str} |\n"));
        }
        out.push('\n');
    }

    // Subcommands list
    if !subcommands.is_empty() {
        out.push_str("**Subcommands:**\n\n");
        for sub in &subcommands {
            let sub_name = sub.get_name();
            let sub_about = sub.get_about().map(|a| a.to_string()).unwrap_or_default();
            out.push_str(&format!("- `{sub_name}` — {sub_about}\n"));
        }
        out.push('\n');
    }

    out.push_str("---\n\n");

    // Recurse into subcommands
    for sub in &subcommands {
        let sub_path = format!("{full_path} {}", sub.get_name());
        render_command(out, sub, &sub_path, heading_level + 1);
    }
}

/// Execute the `wh reference` command.
pub fn execute(args: &ReferenceArgs) {
    let reference = generate_cli_reference();

    match args.format {
        OutputFormat::Human => {
            print!("{reference}");
        }
        OutputFormat::Json => {
            use crate::output::OutputEnvelope;
            #[derive(serde::Serialize)]
            struct ReferenceData {
                content: String,
                format: String,
            }
            let data = ReferenceData {
                content: reference,
                format: "markdown".to_string(),
            };
            let envelope = OutputEnvelope::ok(data);
            if let Ok(json) = serde_json::to_string_pretty(&envelope) {
                println!("{json}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_contains_all_top_level_commands() {
        let reference = generate_cli_reference();
        let cmd = crate::Cli::command();

        for sub in cmd.get_subcommands() {
            if sub.get_name() == "help" {
                continue;
            }
            assert!(
                reference.contains(&format!("`wh {}`", sub.get_name())),
                "Reference should contain command 'wh {}' but it does not.\nReference:\n{}",
                sub.get_name(),
                &reference[..500.min(reference.len())]
            );
        }
    }

    #[test]
    fn reference_contains_exit_code_table() {
        let reference = generate_cli_reference();
        assert!(
            reference.contains("## Exit Codes"),
            "Reference should contain exit codes section"
        );
        assert!(
            reference.contains("| 0 |"),
            "Reference should document exit code 0"
        );
        assert!(
            reference.contains("| 1 |"),
            "Reference should document exit code 1"
        );
        assert!(
            reference.contains("| 2 |"),
            "Reference should document exit code 2"
        );
    }

    #[test]
    fn reference_is_valid_markdown() {
        let reference = generate_cli_reference();
        // Must start with a heading
        assert!(
            reference.starts_with("# "),
            "Reference should start with a markdown heading"
        );
        // Must contain table of contents
        assert!(
            reference.contains("## Table of Contents"),
            "Reference should contain table of contents"
        );
        // Must contain version
        assert!(
            reference.contains("**Version:**"),
            "Reference should contain version info"
        );
    }

    #[test]
    fn reference_documents_subcommands() {
        let reference = generate_cli_reference();
        // Topology has subcommands — verify they are documented
        assert!(
            reference.contains("`wh topology lint`"),
            "Reference should document 'wh topology lint'"
        );
        assert!(
            reference.contains("`wh topology plan`"),
            "Reference should document 'wh topology plan'"
        );
        assert!(
            reference.contains("`wh topology apply`"),
            "Reference should document 'wh topology apply'"
        );
        assert!(
            reference.contains("`wh topology destroy`"),
            "Reference should document 'wh topology destroy'"
        );
    }

    #[test]
    fn reference_documents_arguments_and_flags() {
        let reference = generate_cli_reference();
        // The topology lint command has a `file` argument and `--format` flag
        assert!(
            reference.contains("`file`"),
            "Reference should document positional arguments"
        );
        assert!(
            reference.contains("`--format`"),
            "Reference should document --format flag"
        );
    }

    #[test]
    fn reference_covers_stream_subcommands() {
        let reference = generate_cli_reference();
        let cmd = crate::Cli::command();

        // Find the stream command and check all its subcommands are present
        for sub in cmd.get_subcommands() {
            if sub.get_name() == "help" {
                continue;
            }
            for nested in sub.get_subcommands() {
                if nested.get_name() == "help" {
                    continue;
                }
                let path = format!("`wh {} {}`", sub.get_name(), nested.get_name());
                assert!(
                    reference.contains(&path),
                    "Reference should contain subcommand '{path}'"
                );
            }
        }
    }

    #[test]
    fn reference_contains_changes_detected_exit_code() {
        let reference = generate_cli_reference();
        assert!(
            reference.contains("Changes detected"),
            "Exit code table should explain code 2 as 'Changes detected'"
        );
    }
}
