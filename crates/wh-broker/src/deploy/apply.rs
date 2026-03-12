//! Apply step of the deploy pipeline.
//!
//! Consumes a `PlanOutput` via `commit()` to produce a `CommittedPlan`,
//! then `apply()` consumes the `CommittedPlan` to finalize the deployment.
//!
//! The commit step performs the git commit with plan_hash in the message body (ADR-003).
//! The apply step persists the new topology state to `.wh/state.json`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::deploy::plan::PlanOutput;
use crate::deploy::{DeployError, Topology};

/// Default timeout for git subprocess calls (CM-04).
const GIT_TIMEOUT: Duration = Duration::from_secs(30);

/// A committed plan ready for application.
///
/// This typestate token proves that the plan has been committed to git.
/// It must be consumed by `apply()`.
#[must_use = "a CommittedPlan must be passed to apply() — do not discard"]
#[derive(Debug)]
pub struct CommittedPlan {
    #[allow(dead_code)]
    pub(crate) desired_topology: Topology,
    #[allow(dead_code)]
    pub(crate) source_path: PathBuf,
    pub(crate) plan_hash: String,
}

impl CommittedPlan {
    pub fn plan_hash(&self) -> &str {
        &self.plan_hash
    }
}

/// Find the git binary, checking common paths.
fn find_git() -> &'static str {
    // Try common paths for git
    for path in &["/usr/bin/git", "/usr/local/bin/git", "/opt/homebrew/bin/git"] {
        if std::path::Path::new(path).exists() {
            // Leak to get 'static — only called a few times
            return path;
        }
    }
    // Fallback: rely on PATH
    "git"
}

/// Run a git command with a 30s timeout (CM-04).
///
/// Spawns git as a subprocess and polls for completion. If the timeout
/// fires, the process is killed and `DeployError::GitTimeout` is returned.
fn run_git(
    workspace_root: &Path,
    args: &[&str],
) -> Result<std::process::Output, DeployError> {
    let git = find_git();
    let mut child = Command::new(git)
        .args(args)
        .current_dir(workspace_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| DeployError::GitFailed(format!("failed to spawn git: {e}")))?;

    // Poll for completion with timeout (CM-04)
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child.wait_with_output().map_err(|e| {
                    DeployError::GitFailed(format!("git process error: {e}"))
                });
            }
            Ok(None) => {
                if start.elapsed() > GIT_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(DeployError::GitTimeout(GIT_TIMEOUT.as_secs()));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                return Err(DeployError::GitFailed(format!("git process error: {e}")));
            }
        }
    }
}

/// Run a git command and check that it succeeded.
fn run_git_checked(
    workspace_root: &Path,
    args: &[&str],
) -> Result<std::process::Output, DeployError> {
    let output = run_git(workspace_root, args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DeployError::GitFailed(format!(
            "git {} failed: {stderr}",
            args.first().unwrap_or(&"")
        )));
    }
    Ok(output)
}

/// Generate the change summary for the commit message.
fn change_summary(plan: &PlanOutput) -> String {
    let mut parts = Vec::new();
    for change in plan.changes() {
        match change.op.as_str() {
            "+" => parts.push(format!("add {}", change.component)),
            "-" => parts.push(format!("remove {}", change.component)),
            "~" => {
                if let Some(field) = &change.field {
                    parts.push(format!("update {} {}", change.component, field));
                } else {
                    parts.push(format!("update {}", change.component));
                }
            }
            _ => parts.push(format!("{} {}", change.op, change.component)),
        }
    }
    if parts.is_empty() {
        "no changes".to_string()
    } else {
        parts.join(", ")
    }
}

/// Commit the plan to git, producing a `CommittedPlan`.
///
/// The git commit message follows ADR-003 format:
/// `[agent-name] apply: <summary>\n\nPlan: <plan_hash>`
///
/// In operator mode (default), the agent name is "operator".
#[tracing::instrument(skip_all, fields(topology = %plan.topology_name(), hash = %plan.plan_hash()))]
pub fn commit(plan: PlanOutput, agent_name: Option<&str>) -> Result<CommittedPlan, DeployError> {
    let agent = agent_name.unwrap_or("operator");
    let summary = change_summary(&plan);
    let plan_hash = plan.plan_hash().to_string();

    let workspace_root = plan
        .source_path()
        .parent()
        .unwrap_or_else(|| Path::new("."));

    // Write the desired topology state to .wh/state.json
    let wh_dir = workspace_root.join(".wh");
    std::fs::create_dir_all(&wh_dir).map_err(DeployError::FileRead)?;

    let state_path = wh_dir.join("state.json");
    let state_json = serde_json::to_string_pretty(&plan.desired_topology)
        .map_err(|e| DeployError::ApplyFailed(format!("failed to serialize state: {e}")))?;
    std::fs::write(&state_path, &state_json).map_err(DeployError::FileRead)?;

    let wh_file_name = plan
        .source_path()
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("topology.wh"));

    // Stage the state file and topology file — check for errors
    let stage_result = (|| -> Result<(), DeployError> {
        run_git_checked(workspace_root, &["add", ".wh/state.json"])?;
        run_git_checked(workspace_root, &["add", &wh_file_name.to_string_lossy()])?;
        Ok(())
    })();

    if let Err(e) = stage_result {
        // Cleanup: remove state file to avoid corrupting future plans
        let _ = std::fs::remove_file(&state_path);
        return Err(e);
    }

    // Commit with ADR-003 format
    let commit_message = format!("[{agent}] apply: {summary}\n\nPlan: {plan_hash}");
    let commit_result = run_git(workspace_root, &["commit", "-m", &commit_message])?;

    if !commit_result.status.success() {
        // Cleanup: remove state file on commit failure
        let _ = std::fs::remove_file(&state_path);
        let stderr = String::from_utf8_lossy(&commit_result.stderr);
        return Err(DeployError::GitFailed(format!(
            "git commit failed: {stderr}"
        )));
    }

    Ok(CommittedPlan {
        desired_topology: plan.desired_topology,
        source_path: plan.source_path,
        plan_hash,
    })
}

/// Apply the committed plan: finalize the deployment.
///
/// This step persists the topology state. Since we already wrote state.json
/// during commit, this step is a no-op in MVP (FM-04: idempotent).
/// Future stories will add broker state updates here.
#[tracing::instrument(skip_all)]
pub fn apply(committed: CommittedPlan) -> Result<(), DeployError> {
    // State already persisted in commit step.
    // Future: broker state update goes here.
    // FM-04: Idempotent — applying same .wh twice = same result.
    let _ = committed; // consume the token
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_summary_formats_correctly() {
        let plan = PlanOutput {
            has_changes: true,
            changes: vec![
                crate::deploy::Change {
                    op: "~".to_string(),
                    component: "agent researcher".to_string(),
                    field: Some("replicas".to_string()),
                    from: Some(serde_json::json!(1)),
                    to: Some(serde_json::json!(2)),
                },
            ],
            plan_hash: "sha256:abc".to_string(),
            topology_name: "dev".to_string(),
            policy_snapshot_hash: String::new(),
            warnings: vec![],
            desired_topology: Topology {
                api_version: "wheelhouse.dev/v1".to_string(),
                name: "dev".to_string(),
                agents: vec![],
                streams: vec![],
                guardrails: None,
            },
            source_path: PathBuf::from("test.wh"),
        };

        let summary = change_summary(&plan);
        assert_eq!(summary, "update agent researcher replicas");
    }

    #[test]
    fn change_summary_handles_additions() {
        let plan = PlanOutput {
            has_changes: true,
            changes: vec![
                crate::deploy::Change {
                    op: "+".to_string(),
                    component: "agent donna".to_string(),
                    field: None,
                    from: None,
                    to: None,
                },
            ],
            plan_hash: "sha256:abc".to_string(),
            topology_name: "dev".to_string(),
            policy_snapshot_hash: String::new(),
            warnings: vec![],
            desired_topology: Topology {
                api_version: "wheelhouse.dev/v1".to_string(),
                name: "dev".to_string(),
                agents: vec![],
                streams: vec![],
                guardrails: None,
            },
            source_path: PathBuf::from("test.wh"),
        };

        let summary = change_summary(&plan);
        assert_eq!(summary, "add agent donna");
    }
}
