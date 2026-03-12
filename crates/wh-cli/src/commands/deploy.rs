//! `wh deploy` subcommand group: apply/plan/lint/destroy (CRG-07).
//!
//! This file contains all deploy subcommands per CRG-07:
//! "never create a new file for a subcommand of an existing category".

use std::path::PathBuf;

use clap::Subcommand;

use crate::lint::{self, LintResult};
use crate::output::{Format, LintError, OutputEnvelope, WhError};

#[derive(Debug, Subcommand)]
pub enum DeployCmd {
    /// Validate the syntax of a `.wh` file without deploying anything (FR57).
    Lint {
        /// Path to the `.wh` file to validate.
        file: PathBuf,
    },
    // [PHASE-2-ONLY: DEPLOY-PLAN] plan subcommand (story 2-3)
    // [PHASE-2-ONLY: DEPLOY-APPLY] apply subcommand (story 2-4)
    // [PHASE-2-ONLY: DEPLOY-DESTROY] destroy subcommand (story 2-7)
}

/// Run a deploy subcommand. Returns exit code.
pub fn run(cmd: DeployCmd, format: Format) -> Result<i32, WhError> {
    match cmd {
        DeployCmd::Lint { file } => run_lint(&file, format),
    }
}

/// Emit a pre-lint error (file read or YAML parse failure) in the appropriate format.
fn emit_early_error(format: Format, code: &str, message: &str) {
    match format {
        Format::Human => {
            eprintln!("{message}");
        }
        Format::Json => {
            let envelope = crate::output::ErrorEnvelope::new(code, message);
            let json = serde_json::to_string_pretty(&envelope)
                .expect("ErrorEnvelope serialization should not fail");
            println!("{json}");
        }
    }
}

fn run_lint(file: &std::path::Path, format: Format) -> Result<i32, WhError> {
    let (result, _linted) = match lint::lint_file(file) {
        Ok(r) => r,
        Err(LintError::FileReadError(e)) => {
            emit_early_error(
                format,
                "LINT_FILE_ERROR",
                &format!("cannot read '{}': {e}", file.display()),
            );
            return Ok(1);
        }
        Err(LintError::YamlParseError(detail)) => {
            emit_early_error(
                format,
                "LINT_PARSE_ERROR",
                &format!("YAML parse error: {detail}"),
            );
            return Ok(1);
        }
    };

    match format {
        Format::Human => print_human(&result),
        Format::Json => print_json(&result),
    }

    if result.has_errors() {
        Ok(1)
    } else {
        Ok(0)
    }
}

fn print_human(result: &LintResult) {
    for diag in &result.errors {
        eprintln!("{diag}");
    }
    for diag in &result.warnings {
        eprintln!("{diag}");
    }
}

/// Lint JSON data payload (inside the envelope).
#[derive(serde::Serialize)]
struct LintJsonData {
    errors: Vec<lint::LintDiagnostic>,
    warnings: Vec<lint::LintDiagnostic>,
}

fn print_json(result: &LintResult) {
    let data = LintJsonData {
        errors: result.errors.clone(),
        warnings: result.warnings.clone(),
    };

    let envelope = if result.has_errors() {
        OutputEnvelope::error(data)
    } else {
        OutputEnvelope::ok(data)
    };

    let json = serde_json::to_string_pretty(&envelope).expect("JSON serialization should not fail");
    println!("{json}");
}
