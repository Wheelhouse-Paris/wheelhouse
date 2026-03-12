//! `wh deploy` subcommands: lint, plan, apply.
//!
//! Implements the operator-facing CLI for the deploy pipeline.
//! All output routed through the format switch (SCV-05).

use std::path::PathBuf;

use clap::Subcommand;
use wh_broker::deploy::lint;
use wh_broker::deploy::plan::{self, PlanData};
use wh_broker::deploy::apply;

use crate::lint as lint_engine;
use crate::output::{self, LintError, OutputFormat};
use crate::output::error;

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
}

impl DeployCommand {
    /// Execute the deploy subcommand. Returns the process exit code.
    pub fn execute(self) -> i32 {
        match self {
            DeployCommand::Lint { file, format } => execute_lint(&file, format),
            DeployCommand::Plan { file, format } => execute_plan(&file, format),
            DeployCommand::Apply { file, yes, format, agent_name } => {
                execute_apply(&file, yes, format, agent_name.as_deref())
            }
        }
    }
}

fn execute_lint(file: &PathBuf, format: OutputFormat) -> i32 {
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
                let envelope = output::OutputEnvelope::<()>::error("LINT_ERROR", "lint validation failed");
                if let Ok(json) = serde_json::to_string_pretty(&envelope) {
                    println!("{json}");
                }
            } else {
                let envelope = output::OutputEnvelope::ok(data);
                if let Ok(json) = serde_json::to_string_pretty(&envelope) {
                    println!("{json}");
                }
            }
        }
    }

    if result.has_errors() { error::EXIT_ERROR } else { error::EXIT_SUCCESS }
}

fn execute_plan(file: &PathBuf, format: OutputFormat) -> i32 {
    let linted = match lint::lint(file) {
        Ok(l) => l,
        Err(e) => {
            let msg = output::format_error(e.code(), &e.to_string(), format);
            eprintln!("{msg}");
            return error::EXIT_ERROR;
        }
    };

    let plan_output = match plan::plan(linted) {
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

fn execute_apply(file: &PathBuf, yes: bool, format: OutputFormat, agent_name: Option<&str>) -> i32 {
    if !yes {
        eprintln!("Error: interactive confirmation not yet supported. Use --yes to skip.");
        return error::EXIT_ERROR;
    }

    let linted = match lint::lint(file) {
        Ok(l) => l,
        Err(e) => {
            let msg = output::format_error(e.code(), &e.to_string(), format);
            eprintln!("{msg}");
            return error::EXIT_ERROR;
        }
    };

    let plan_output = match plan::plan(linted) {
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

    let committed = match apply::commit(plan_output, agent_name) {
        Ok(c) => c,
        Err(e) => {
            let msg = output::format_error(e.code(), &e.to_string(), format);
            eprintln!("{msg}");
            return error::EXIT_ERROR;
        }
    };

    if let Err(e) = apply::apply(committed) {
        let msg = output::format_error(e.code(), &e.to_string(), format);
        eprintln!("{msg}");
        return error::EXIT_ERROR;
    }

    match format {
        OutputFormat::Human => println!("Topology applied successfully."),
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "v": 1,
                    "status": "ok",
                    "data": { "applied": true }
                })
            );
        }
    }

    error::EXIT_SUCCESS
}
