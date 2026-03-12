//! `wh deploy` subcommands: lint, plan, apply.
//!
//! Implements the operator-facing CLI for the deploy pipeline.
//! All output routed through the format switch (SCV-05).

use std::path::PathBuf;

use clap::Subcommand;
use wh_broker::deploy::apply;
use wh_broker::deploy::lint;
use wh_broker::deploy::plan::{self, PlanData};

use crate::lint as lint_engine;
use crate::output::error;
use crate::output::{self, LintError, OutputFormat};

/// Deploy subcommands for managing topology.
#[derive(Debug, Subcommand)]
pub enum DeployCommand {
    /// Validate the syntax and semantics of a `.wh` topology file.
    Lint {
        /// Path to the `.wh` file to validate.
        file: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Preview changes before applying a topology
    Plan {
        /// Path to the .wh topology file
        file: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
        /// Allow plans that would destroy all agents (self-destruct safety override, CM-05)
        #[arg(long)]
        force_destroy_all: bool,
        /// Agent name for self-destruct detection (CM-05). When provided,
        /// the plan will be rejected if it would remove this agent.
        #[arg(long)]
        calling_agent: Option<String>,
    },
    /// Apply a topology, optionally without prompting
    Apply {
        /// Path to the .wh topology file
        file: PathBuf,
        /// Skip interactive confirmation
        #[arg(long)]
        yes: bool,
        /// Output format
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
        /// Agent name for git commit attribution (defaults to "operator")
        #[arg(long)]
        agent_name: Option<String>,
    },
    /// Destroy a deployed topology — stop all containers and clear state (FR3)
    Destroy {
        /// Path to the .wh topology file
        file: PathBuf,
        /// Skip interactive confirmation
        #[arg(long)]
        yes: bool,
        /// Output format
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
        /// Agent name for git commit attribution (defaults to "operator")
        #[arg(long)]
        agent_name: Option<String>,
    },
}

impl DeployCommand {
    /// Execute the deploy subcommand. Returns the process exit code.
    pub fn execute(self) -> i32 {
        match self {
            DeployCommand::Lint { file, format } => execute_lint(&file, format),
            DeployCommand::Plan {
                file,
                format,
                force_destroy_all,
                calling_agent,
            } => execute_plan(&file, format, force_destroy_all, calling_agent.as_deref()),
            DeployCommand::Apply {
                file,
                yes,
                format,
                agent_name,
            } => execute_apply(&file, yes, format, agent_name.as_deref()),
            DeployCommand::Destroy {
                file,
                yes,
                format,
                agent_name,
            } => execute_destroy(&file, yes, format, agent_name.as_deref()),
        }
    }
}

fn execute_lint(file: &std::path::Path, format: OutputFormat) -> i32 {
    let (result, _linted) = match lint_engine::lint_file(file) {
        Ok(r) => r,
        Err(LintError::FileReadError(e)) => {
            let msg = output::format_error(
                "LINT_FILE_ERROR",
                &format!("cannot read '{}': {e}", file.display()),
                format,
            );
            eprintln!("{msg}");
            return error::EXIT_ERROR;
        }
        Err(LintError::YamlParseError(detail)) => {
            let msg = output::format_error(
                "LINT_PARSE_ERROR",
                &format!("YAML parse error: {detail}"),
                format,
            );
            eprintln!("{msg}");
            return error::EXIT_ERROR;
        }
    };

    match format {
        OutputFormat::Human => {
            for diag in &result.errors {
                eprintln!("{diag}");
            }
            for diag in &result.warnings {
                eprintln!("{diag}");
            }
            if result.errors.is_empty() && result.warnings.is_empty() {
                println!("{}: OK", file.display());
            }
        }
        OutputFormat::Json => {
            use serde::Serialize;
            #[derive(Serialize)]
            struct LintJsonData {
                errors: Vec<crate::lint::LintDiagnostic>,
                warnings: Vec<crate::lint::LintDiagnostic>,
            }
            let data = LintJsonData {
                errors: result.errors.clone(),
                warnings: result.warnings.clone(),
            };
            if result.has_errors() {
                // Include lint diagnostics in data even for error status,
                // so consumers can inspect specific errors.
                let envelope = serde_json::json!({
                    "v": 1,
                    "status": "error",
                    "code": "LINT_ERROR",
                    "message": "lint validation failed",
                    "data": {
                        "errors": result.errors,
                        "warnings": result.warnings,
                    }
                });
                println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
            } else {
                let envelope = output::OutputEnvelope::ok(data);
                if let Ok(json) = serde_json::to_string_pretty(&envelope) {
                    println!("{json}");
                }
            }
        }
    }

    if result.has_errors() {
        error::EXIT_ERROR
    } else {
        error::EXIT_SUCCESS
    }
}

fn execute_plan(
    file: &PathBuf,
    format: OutputFormat,
    force_destroy_all: bool,
    calling_agent: Option<&str>,
) -> i32 {
    let linted = match lint::lint(file) {
        Ok(l) => l,
        Err(e) => {
            let msg = output::format_error(e.code(), &e.to_string(), format);
            eprintln!("{msg}");
            return error::EXIT_ERROR;
        }
    };

    let plan_output = match if calling_agent.is_some() {
        plan::plan_with_self_check(linted, calling_agent)
    } else {
        plan::plan_with_options(linted, force_destroy_all)
    } {
        Ok(p) => p,
        Err(e) => {
            let msg = output::format_error(e.code(), &e.to_string(), format);
            eprintln!("{msg}");
            return error::EXIT_ERROR;
        }
    };

    let has_changes = plan_output.has_changes();
    let plan_data = PlanData::from(&plan_output);
    let response = output::format_response(&plan_data, format);
    println!("{response}");

    if has_changes {
        error::EXIT_PLAN_CHANGE
    } else {
        error::EXIT_SUCCESS
    }
}

/// Check if a file descriptor is a TTY.
fn is_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

/// Prompt the user for confirmation. Returns true if user confirms.
fn prompt_confirmation() -> bool {
    use std::io::{BufRead, Write};

    eprint!("Apply changes? (yes/no) ");
    let _ = std::io::stderr().flush();

    let stdin = std::io::stdin();
    let mut line = String::new();
    if stdin.lock().read_line(&mut line).is_err() {
        return false;
    }
    line.trim().eq_ignore_ascii_case("yes")
}

/// Simple progress indicator — writes to stderr.
fn progress_step(msg: &str, is_tty: bool) {
    if is_tty {
        eprint!("\r\x1b[K{msg}");
        let _ = std::io::Write::flush(&mut std::io::stderr());
    } else {
        eprintln!("{msg}");
    }
}

/// Clear the progress line (TTY only).
fn progress_done(msg: &str, is_tty: bool) {
    if is_tty {
        eprintln!("\r\x1b[K{msg}");
    } else {
        eprintln!("{msg}");
    }
}

fn execute_apply(file: &PathBuf, yes: bool, format: OutputFormat, agent_name: Option<&str>) -> i32 {
    let tty = is_tty();

    // AC #3: Non-interactive mode requires --yes
    if !yes {
        if !tty {
            let msg = output::format_error(
                "APPLY_NO_CONFIRM",
                "cannot prompt for confirmation in non-interactive mode. Use --yes to skip.",
                format,
            );
            eprintln!("{msg}");
            return error::EXIT_ERROR;
        }
    }

    let linted = match lint::lint(file) {
        Ok(l) => l,
        Err(e) => {
            let msg = output::format_error(e.code(), &e.to_string(), format);
            eprintln!("{msg}");
            return error::EXIT_ERROR;
        }
    };

    // Use plan_with_options to ensure self-destruct detection (CM-05)
    let plan_output = match plan::plan_with_options(linted, false) {
        Ok(p) => p,
        Err(e) => {
            let msg = output::format_error(e.code(), &e.to_string(), format);
            eprintln!("{msg}");
            return error::EXIT_ERROR;
        }
    };

    if !plan_output.has_changes() {
        match format {
            OutputFormat::Human => println!("No changes to apply. Topology is up to date."),
            OutputFormat::Json => {
                let plan_data = PlanData::from(&plan_output);
                println!("{}", output::format_response(&plan_data, format));
            }
        }
        return error::EXIT_SUCCESS;
    }

    // AC #1: Show plan and prompt for confirmation (unless --yes)
    if !yes {
        let plan_data = PlanData::from(&plan_output);
        eprintln!("{plan_data}");

        if !prompt_confirmation() {
            eprintln!("Apply cancelled.");
            return error::EXIT_SUCCESS;
        }
    }

    // Commit the plan to git
    progress_step("Committing topology changes...", tty);
    let committed = match apply::commit(plan_output, agent_name) {
        Ok(c) => c,
        Err(e) => {
            progress_done("", tty);
            let msg = output::format_error(e.code(), &e.to_string(), format);
            eprintln!("{msg}");
            return error::EXIT_ERROR;
        }
    };
    progress_done("Topology changes committed.", tty);

    // AC #1: Spinner during provisioning
    progress_step("Provisioning containers...", tty);
    let apply_result = match apply::apply(committed) {
        Ok(r) => r,
        Err(e) => {
            progress_done("", tty);
            let msg = output::format_error(e.code(), &e.to_string(), format);
            eprintln!("{msg}");
            return error::EXIT_ERROR;
        }
    };
    progress_done("Provisioning complete.", tty);

    // AC #2: Summary line
    match format {
        OutputFormat::Human => {
            println!("{apply_result}");
        }
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "v": 1,
                    "status": "ok",
                    "data": {
                        "applied": true,
                        "created": apply_result.created,
                        "changed": apply_result.changed,
                        "destroyed": apply_result.destroyed
                    }
                })
            );
        }
    }

    error::EXIT_SUCCESS
}

fn execute_destroy(
    file: &PathBuf,
    yes: bool,
    format: OutputFormat,
    agent_name: Option<&str>,
) -> i32 {
    let tty = is_tty();

    // Require --yes in non-interactive mode (matches execute_apply pattern)
    if !yes && !tty {
        let msg = output::format_error(
            "DESTROY_NO_CONFIRM",
            "cannot prompt for confirmation in non-interactive mode. Use --yes to skip.",
            format,
        );
        eprintln!("{msg}");
        return error::EXIT_ERROR;
    }

    // Validate the topology file exists and is parseable
    if let Err(e) = lint::lint(file) {
        let msg = output::format_error(e.code(), &e.to_string(), format);
        eprintln!("{msg}");
        return error::EXIT_ERROR;
    }

    // Show what will be destroyed and prompt for confirmation
    if !yes {
        eprintln!(
            "This will destroy all deployed components from topology '{}'.",
            file.display()
        );
        eprintln!("All Podman containers will be stopped and removed.");
        eprintln!();

        if !prompt_confirmation() {
            eprintln!("Destroy cancelled.");
            return error::EXIT_SUCCESS;
        }
    }

    // Execute destroy
    progress_step("Destroying topology...", tty);
    let destroy_result = match apply::destroy(file, agent_name) {
        Ok(r) => r,
        Err(e) => {
            progress_done("", tty);
            let msg = output::format_error(e.code(), &e.to_string(), format);
            eprintln!("{msg}");
            return error::EXIT_ERROR;
        }
    };
    progress_done("Topology destroyed.", tty);

    // Output result
    match format {
        OutputFormat::Human => {
            if destroy_result.destroyed == 0 && destroy_result.streams_removed == 0 {
                println!("Nothing to destroy. No components were deployed.");
            } else {
                println!("{destroy_result}");
            }
        }
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "v": 1,
                    "status": "ok",
                    "data": {
                        "destroyed": true,
                        "agents_removed": destroy_result.destroyed,
                        "streams_removed": destroy_result.streams_removed
                    }
                })
            );
        }
    }

    error::EXIT_SUCCESS
}
