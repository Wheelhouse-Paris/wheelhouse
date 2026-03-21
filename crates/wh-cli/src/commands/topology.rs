//! `wh topology` subcommands: lint, plan, apply.
//!
//! Implements the operator-facing CLI for the topology pipeline.
//! All output routed through the format switch (SCV-05).
//! Supports both single `.wh` file paths and folders containing
//! multiple `.wh` files (ADR-030: folder-based topology composition).

use std::path::PathBuf;

use clap::Subcommand;
use wh_broker::deploy::apply;
use wh_broker::deploy::lint;
use wh_broker::deploy::plan::{self, PlanData};

use crate::lint as lint_engine;
use crate::output::error;
use crate::output::{self, LintError, OutputFormat};

/// Topology subcommands: lint, plan, apply, destroy.
#[derive(Debug, Subcommand)]
pub enum TopologyCommand {
    /// Validate the syntax and semantics of a `.wh` topology file or folder.
    Lint {
        /// Path to a `.wh` file or folder containing `.wh` files.
        path: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Preview changes before applying a topology
    Plan {
        /// Path to a `.wh` file or folder containing `.wh` files.
        path: PathBuf,
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
        /// Path to a `.wh` file or folder containing `.wh` files.
        path: PathBuf,
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
        /// Path to a `.wh` file or folder containing `.wh` files.
        path: PathBuf,
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

impl TopologyCommand {
    /// Execute the topology subcommand. Returns the process exit code.
    pub fn execute(self) -> i32 {
        match self {
            TopologyCommand::Lint { path, format } => execute_lint(&path, format),
            TopologyCommand::Plan {
                path,
                format,
                force_destroy_all,
                calling_agent,
            } => execute_plan(&path, format, force_destroy_all, calling_agent.as_deref()),
            TopologyCommand::Apply {
                path,
                yes,
                format,
                agent_name,
            } => execute_apply(&path, yes, format, agent_name.as_deref()),
            TopologyCommand::Destroy {
                path,
                yes,
                format,
                agent_name,
            } => execute_destroy(&path, yes, format, agent_name.as_deref()),
        }
    }
}

fn execute_lint(path: &std::path::Path, format: OutputFormat) -> i32 {
    // If path is a directory, use folder-based lint (ADR-030, E12-02)
    if path.is_dir() {
        return execute_lint_folder(path, format);
    }

    let (result, _linted) = match lint_engine::lint_file(path) {
        Ok(r) => r,
        Err(LintError::FileReadError(e)) => {
            let msg = output::format_error(
                "LINT_FILE_ERROR",
                &format!("cannot read '{}': {e}", path.display()),
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
                println!("{}: OK", path.display());
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

/// Lint a folder containing multiple `.wh` files (ADR-030, E12-02).
///
/// 1. Lint each `.wh` file individually — report per-file errors first.
/// 2. If all files pass: validate the merged graph (cross-file duplicate detection).
fn execute_lint_folder(dir: &std::path::Path, format: OutputFormat) -> i32 {
    // Discover *.wh files
    let mut wh_files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                if path.is_file() && path.extension().is_some_and(|ext| ext == "wh") {
                    Some(path)
                } else {
                    None
                }
            })
            .collect(),
        Err(e) => {
            let msg = output::format_error(
                "LINT_FILE_ERROR",
                &format!("cannot read directory '{}': {e}", dir.display()),
                format,
            );
            eprintln!("{msg}");
            return error::EXIT_ERROR;
        }
    };

    if wh_files.is_empty() {
        let msg = output::format_error(
            "LINT_NO_FILES",
            &format!("no .wh files found in folder '{}'", dir.display()),
            format,
        );
        eprintln!("{msg}");
        return error::EXIT_ERROR;
    }

    wh_files.sort_by(|a, b| {
        a.file_name()
            .unwrap_or_default()
            .cmp(b.file_name().unwrap_or_default())
    });

    // Phase 1: Lint each file individually
    let mut all_errors = Vec::new();
    let mut all_warnings = Vec::new();
    let mut had_per_file_errors = false;

    for file in &wh_files {
        match lint_engine::lint_file(file) {
            Ok((result, _linted)) => {
                all_errors.extend(result.errors.clone());
                all_warnings.extend(result.warnings.clone());
                if result.has_errors() {
                    had_per_file_errors = true;
                }
            }
            Err(LintError::FileReadError(e)) => {
                let filename = file
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| file.to_string_lossy().to_string());
                all_errors.push(crate::lint::LintDiagnostic {
                    file: filename,
                    line: None,
                    level: crate::lint::DiagnosticLevel::Error,
                    message: format!("cannot read file: {e}"),
                    hint: "check file permissions".to_string(),
                });
                had_per_file_errors = true;
            }
            Err(LintError::YamlParseError(detail)) => {
                let filename = file
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| file.to_string_lossy().to_string());
                all_errors.push(crate::lint::LintDiagnostic {
                    file: filename,
                    line: None,
                    level: crate::lint::DiagnosticLevel::Error,
                    message: format!("YAML parse error: {detail}"),
                    hint: "fix YAML syntax".to_string(),
                });
                had_per_file_errors = true;
            }
        }
    }

    // Phase 2: If per-file lint passed, validate merged graph (cross-file duplicates)
    if !had_per_file_errors {
        // Use broker-level merge which validates apiVersion consistency,
        // name consistency, and detects duplicates
        if let Err(e) = lint::lint(dir) {
            all_errors.push(crate::lint::LintDiagnostic {
                file: dir.to_string_lossy().to_string(),
                line: None,
                level: crate::lint::DiagnosticLevel::Error,
                message: e.to_string(),
                hint: "fix cross-file conflicts".to_string(),
            });
        }
    }

    // Output results
    let has_errors = !all_errors.is_empty();

    match format {
        OutputFormat::Human => {
            for diag in &all_errors {
                eprintln!("{diag}");
            }
            for diag in &all_warnings {
                eprintln!("{diag}");
            }
            if !has_errors && all_warnings.is_empty() {
                println!("{}: OK ({} files)", dir.display(), wh_files.len());
            }
        }
        OutputFormat::Json => {
            if has_errors {
                let envelope = serde_json::json!({
                    "v": 1,
                    "status": "error",
                    "code": "LINT_ERROR",
                    "message": "lint validation failed",
                    "data": {
                        "errors": all_errors,
                        "warnings": all_warnings,
                    }
                });
                println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
            } else {
                use serde::Serialize;
                #[derive(Serialize)]
                struct LintJsonData {
                    errors: Vec<crate::lint::LintDiagnostic>,
                    warnings: Vec<crate::lint::LintDiagnostic>,
                    files_count: usize,
                }
                let data = LintJsonData {
                    errors: all_errors,
                    warnings: all_warnings,
                    files_count: wh_files.len(),
                };
                let envelope = output::OutputEnvelope::ok(data);
                if let Ok(json) = serde_json::to_string_pretty(&envelope) {
                    println!("{json}");
                }
            }
        }
    }

    if has_errors {
        error::EXIT_ERROR
    } else {
        error::EXIT_SUCCESS
    }
}

fn execute_plan(
    path: &PathBuf,
    format: OutputFormat,
    force_destroy_all: bool,
    calling_agent: Option<&str>,
) -> i32 {
    let linted = match lint::lint(path) {
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

fn execute_apply(path: &PathBuf, yes: bool, format: OutputFormat, agent_name: Option<&str>) -> i32 {
    let tty = is_tty();

    // AC #3: Non-interactive mode requires --yes
    if !yes && !tty {
        let msg = output::format_error(
            "APPLY_NO_CONFIRM",
            "cannot prompt for confirmation in non-interactive mode. Use --yes to skip.",
            format,
        );
        eprintln!("{msg}");
        return error::EXIT_ERROR;
    }

    let linted = match lint::lint(path) {
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

    // Resolve Telegram surfaces regardless of topology changes — state may be missing
    // (e.g. first deploy after state file was deleted, or group migrated to supergroup).
    // For folder-based topologies, use the source_path which is the directory itself.
    let telegram_path = plan_output.source_path();
    let telegram_resolve_path = if telegram_path.is_dir() {
        // For folder-based composition, pick the first .wh file for telegram resolution
        telegram_path.to_path_buf()
    } else {
        telegram_path.to_path_buf()
    };
    let telegram_routing_file =
        match crate::commands::telegram::resolve_telegram_surfaces(&telegram_resolve_path) {
            Ok(path) => path,
            Err(e) => {
                let msg = output::format_error("TELEGRAM_RESOLVE_ERROR", &e, format);
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

    // Collect secrets to inject into agent containers
    let mut extra_env: Vec<(String, String)> = Vec::new();
    if let Some(routing_path) = telegram_routing_file {
        extra_env.push((
            "WH_TELEGRAM_ROUTING_FILE".to_string(),
            routing_path.to_string_lossy().to_string(),
        ));
    }
    for cred in crate::commands::secrets::CREDENTIALS {
        if let Ok(value) = crate::commands::secrets::read_secret(cred.name) {
            extra_env.push((cred.env_var.to_string(), value));
        }
    }

    // AC #1: Spinner during provisioning
    progress_step("Provisioning containers...", tty);
    let apply_result = match apply::apply(committed, &extra_env) {
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
                        "destroyed": apply_result.destroyed,
                        "streams_created": apply_result.streams_created,
                        "surfaces_created": apply_result.surfaces_created,
                        "surfaces_changed": apply_result.surfaces_changed,
                        "surfaces_destroyed": apply_result.surfaces_destroyed
                    }
                })
            );
        }
    }

    error::EXIT_SUCCESS
}

fn execute_destroy(
    path: &PathBuf,
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
    if let Err(e) = lint::lint(path) {
        let msg = output::format_error(e.code(), &e.to_string(), format);
        eprintln!("{msg}");
        return error::EXIT_ERROR;
    }

    // Show what will be destroyed and prompt for confirmation
    if !yes {
        eprintln!(
            "This will destroy all deployed components from topology '{}'.",
            path.display()
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
    let destroy_result = match apply::destroy(path, agent_name) {
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
            if destroy_result.destroyed == 0
                && destroy_result.streams_removed == 0
                && destroy_result.surfaces_destroyed == 0
            {
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
                        "streams_removed": destroy_result.streams_removed,
                        "surfaces_destroyed": destroy_result.surfaces_destroyed
                    }
                })
            );
        }
    }

    error::EXIT_SUCCESS
}
