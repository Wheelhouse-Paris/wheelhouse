//! `wh deploy` subcommands: plan, apply.
//!
//! Implements the operator-facing CLI for the deploy pipeline.
//! All output routed through the format switch (SCV-05).

use std::path::PathBuf;

use clap::Subcommand;
use wh_broker::deploy::lint;
use wh_broker::deploy::plan::{self, PlanData};
use wh_broker::deploy::apply;

use crate::output::{self, OutputFormat};
use crate::output::error;

/// Deploy subcommands for managing topology.
#[derive(Debug, Subcommand)]
pub enum DeployCommand {
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
            DeployCommand::Plan { file, format } => execute_plan(&file, format),
            DeployCommand::Apply { file, yes, format, agent_name } => {
                execute_apply(&file, yes, format, agent_name.as_deref())
            }
        }
    }
}

fn execute_plan(file: &PathBuf, format: OutputFormat) -> i32 {
    // Lint
    let linted = match lint::lint(file) {
        Ok(l) => l,
        Err(e) => {
            let msg = output::format_error(e.code(), &e.to_string(), format);
            eprintln!("{msg}");
            return error::EXIT_ERROR;
        }
    };

    // Plan
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

    // Lint
    let linted = match lint::lint(file) {
        Ok(l) => l,
        Err(e) => {
            let msg = output::format_error(e.code(), &e.to_string(), format);
            eprintln!("{msg}");
            return error::EXIT_ERROR;
        }
    };

    // Plan
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

    // Commit to git
    let committed = match apply::commit(plan_output, agent_name) {
        Ok(c) => c,
        Err(e) => {
            let msg = output::format_error(e.code(), &e.to_string(), format);
            eprintln!("{msg}");
            return error::EXIT_ERROR;
        }
    };

    // Apply
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
