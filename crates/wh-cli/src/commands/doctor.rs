//! `wh doctor` — git repository health check (FM-07).
//!
//! Validates:
//! - Git repository exists
//! - `.wh/.gitignore` exists and contains required exclusion patterns
//! - No secrets found in git history within `.wh/` directory

use std::path::PathBuf;

use clap::Args;
use console::style;
use serde::Serialize;

use crate::output::{OutputEnvelope, OutputFormat};

/// Arguments for the `wh doctor` command.
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Output format: human (default) or json.
    #[arg(long, default_value = "human")]
    pub format: OutputFormat,

    /// Path to workspace root (defaults to current directory).
    #[arg(long, default_value = ".")]
    pub path: PathBuf,
}

/// Result of a single health check.
#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
}

/// Status of a health check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

/// Full doctor report for JSON output.
#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub checks: Vec<CheckResult>,
    pub pass_count: usize,
    pub warn_count: usize,
    pub fail_count: usize,
}

impl DoctorArgs {
    pub fn execute(&self) -> i32 {
        let mut checks = Vec::new();

        // Check 1: Git repository exists
        checks.push(check_git_repo(&self.path));

        // Check 2: .wh/.gitignore exists and is complete
        checks.push(check_gitignore(&self.path));

        // Check 3: Secrets in git history (only if git repo exists)
        if checks[0].status != CheckStatus::Fail {
            checks.push(check_secrets_in_history(&self.path));
        }

        let pass_count = checks
            .iter()
            .filter(|c| c.status == CheckStatus::Pass)
            .count();
        let warn_count = checks
            .iter()
            .filter(|c| c.status == CheckStatus::Warn)
            .count();
        let fail_count = checks
            .iter()
            .filter(|c| c.status == CheckStatus::Fail)
            .count();

        match self.format {
            OutputFormat::Human => {
                println!("Wheelhouse Doctor\n");
                for check in &checks {
                    let icon = match check.status {
                        CheckStatus::Pass => style("PASS").green().bold(),
                        CheckStatus::Warn => style("WARN").yellow().bold(),
                        CheckStatus::Fail => style("FAIL").red().bold(),
                    };
                    println!("  [{}] {} — {}", icon, check.name, check.message);
                }
                println!();
                println!("  {pass_count} passed, {warn_count} warnings, {fail_count} failures");
            }
            OutputFormat::Json => {
                let report = DoctorReport {
                    checks,
                    pass_count,
                    warn_count,
                    fail_count,
                };
                let envelope = OutputEnvelope::ok(report);
                if let Ok(json) = serde_json::to_string_pretty(&envelope) {
                    println!("{json}");
                }
            }
        }

        if fail_count > 0 {
            1
        } else {
            0
        }
    }
}

fn check_git_repo(path: &std::path::Path) -> CheckResult {
    let git_dir = path.join(".git");
    if git_dir.exists() {
        CheckResult {
            name: "Git repository".to_string(),
            status: CheckStatus::Pass,
            message: "Git repository detected".to_string(),
        }
    } else {
        CheckResult {
            name: "Git repository".to_string(),
            status: CheckStatus::Fail,
            message: "No .git directory found — Wheelhouse requires a git repository".to_string(),
        }
    }
}

fn check_gitignore(path: &std::path::Path) -> CheckResult {
    let gitignore_path = path.join(".wh").join(".gitignore");
    if !gitignore_path.exists() {
        return CheckResult {
            name: ".wh/.gitignore".to_string(),
            status: CheckStatus::Warn,
            message: "Missing — run `wh topology apply` to create it automatically".to_string(),
        };
    }

    match wh_broker::deploy::gitignore::check_gitignore_completeness(path) {
        Ok(missing) if missing.is_empty() => CheckResult {
            name: ".wh/.gitignore".to_string(),
            status: CheckStatus::Pass,
            message: "All required exclusion patterns present".to_string(),
        },
        Ok(missing) => CheckResult {
            name: ".wh/.gitignore".to_string(),
            status: CheckStatus::Warn,
            message: format!(
                "Missing {} required pattern(s): {}",
                missing.len(),
                missing.join(", ")
            ),
        },
        Err(e) => CheckResult {
            name: ".wh/.gitignore".to_string(),
            status: CheckStatus::Fail,
            message: format!("Cannot read .wh/.gitignore: {e}"),
        },
    }
}

fn check_secrets_in_history(path: &std::path::Path) -> CheckResult {
    match wh_broker::deploy::gitignore::scan_history_for_secrets(path) {
        Ok(findings) if findings.is_empty() => CheckResult {
            name: "Secrets in history".to_string(),
            status: CheckStatus::Pass,
            message: "No secret patterns found in .wh/ git history".to_string(),
        },
        Ok(findings) => {
            let details: Vec<String> = findings
                .iter()
                .map(|(pattern, count)| format!("'{pattern}' in {count} commit(s)"))
                .collect();
            CheckResult {
                name: "Secrets in history".to_string(),
                status: CheckStatus::Warn,
                message: format!(
                    "Potential secrets found in .wh/ history: {}. Consider running `git filter-branch` to remove them.",
                    details.join(", ")
                ),
            }
        }
        Err(e) => CheckResult {
            name: "Secrets in history".to_string(),
            status: CheckStatus::Warn,
            message: format!("Could not scan history: {e}"),
        },
    }
}
