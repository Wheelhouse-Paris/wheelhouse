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

use crate::deploy::gitignore;
use crate::deploy::plan::PlanOutput;
use crate::deploy::podman::{self, ApplyResult};
use crate::deploy::{Change, DeployError, Topology};

/// Default timeout for git subprocess calls (CM-04).
const GIT_TIMEOUT: Duration = Duration::from_secs(30);

/// A committed plan ready for application.
///
/// This typestate token proves that the plan has been committed to git.
/// It must be consumed by `apply()`.
#[must_use = "a CommittedPlan must be passed to apply() — do not discard"]
#[derive(Debug)]
pub struct CommittedPlan {
    pub(crate) desired_topology: Topology,
    #[allow(dead_code)]
    pub(crate) source_path: PathBuf,
    pub(crate) plan_hash: String,
    pub(crate) changes: Vec<Change>,
}

impl CommittedPlan {
    pub fn plan_hash(&self) -> &str {
        &self.plan_hash
    }
}

/// Find the git binary, checking common paths then $PATH.
pub(crate) fn find_git() -> String {
    // Try common hardcoded paths first (fast path)
    for path in &[
        "/usr/bin/git",
        "/usr/local/bin/git",
        "/opt/homebrew/bin/git",
        "/Library/Developer/CommandLineTools/usr/bin/git",
    ] {
        if std::path::Path::new(path).exists() {
            return path.to_string();
        }
    }
    // Search $PATH at runtime
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            let candidate = std::path::Path::new(dir).join("git");
            if candidate.exists() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    // Last resort: let the OS find it
    "git".to_string()
}

/// Run a git command with a 30s timeout (CM-04).
///
/// Spawns git as a subprocess and polls for completion. If the timeout
/// fires, the process is killed and `DeployError::GitTimeout` is returned.
pub(crate) fn run_git(
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
                return child
                    .wait_with_output()
                    .map_err(|e| DeployError::GitFailed(format!("git process error: {e}")));
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
pub(crate) fn run_git_checked(
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
    let changes = plan.changes().to_vec();

    let workspace_root = plan
        .source_path()
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
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

    // Ensure .wh/.gitignore exists BEFORE staging to prevent WAL/secrets/lock files
    // from being accidentally committed (NFR-S2, FR30).
    gitignore::ensure_gitignore(workspace_root)?;

    // Stage the entire .wh/ directory (state.json, .gitignore, personas, cron,
    // users, compaction summaries) and the topology file (FR28).
    // The .wh/.gitignore excludes WAL, secrets, and lock files automatically.
    let stage_result = (|| -> Result<(), DeployError> {
        run_git_checked(workspace_root, &["add", ".wh/"])?;
        run_git_checked(workspace_root, &["add", &wh_file_name.to_string_lossy()])?;
        Ok(())
    })();

    if let Err(e) = stage_result {
        // Cleanup: remove state file to avoid corrupting future plans
        let _ = std::fs::remove_file(&state_path);
        return Err(e);
    }

    // Pre-commit safety net: scan staged files for accidental secrets (NFR-S2).
    let suspicious = gitignore::scan_staged_for_secrets(workspace_root)?;
    if !suspicious.is_empty() {
        // Unstage everything to prevent partial commit
        let _ = run_git(workspace_root, &["reset", "HEAD"]);
        let _ = std::fs::remove_file(&state_path);
        return Err(DeployError::SecretsDetected(suspicious));
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
        changes,
    })
}

/// Apply the committed plan: provision containers and finalize the deployment.
///
/// ## Recovery Sequence (NFR-I3)
///
/// To restore infrastructure on a new machine:
/// 1. `git clone <repo>` — restores topology config, personas, cron, user profiles
/// 2. `wh broker start` — starts the Wheelhouse broker process
/// 3. `wh deploy apply topology.wh` — provisions agents and streams
///
/// WAL content (in-flight messages) and secrets are NOT restored via git.
/// Secrets must be re-configured via `wh secrets init`.
///
/// State was already persisted to `.wh/state.json` during commit.
/// This step provisions Podman containers for agent changes:
/// - Additions: start new containers
/// - Removals: stop and remove containers
/// - Modifications: restart containers
///
/// Stream changes are no-ops (local provider, handled by broker).
/// FM-04: Idempotent — applying same .wh twice = same result (no changes on second run).
#[tracing::instrument(skip_all)]
pub fn apply(committed: CommittedPlan) -> Result<ApplyResult, DeployError> {
    if committed.changes.is_empty() {
        return Ok(ApplyResult {
            created: 0,
            changed: 0,
            destroyed: 0,
        });
    }

    let workspace_root = committed
        .source_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let result = podman::provision_containers(
        &committed.desired_topology.name,
        &committed.changes,
        &committed.desired_topology.agents,
        Some(workspace_root),
    );

    Ok(result)
}

/// Result of a destroy operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestroyResult {
    /// Number of agent containers destroyed.
    pub destroyed: usize,
    /// Number of streams removed from state (no Podman teardown — streams are broker-managed).
    pub streams_removed: usize,
}

impl std::fmt::Display for DestroyResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} destroyed \u{00B7} {} streams removed",
            self.destroyed, self.streams_removed
        )
    }
}

/// Load the current applied state from `.wh/state.json`.
/// Returns `None` if no state file exists (nothing deployed).
fn load_state(workspace_root: &Path) -> Result<Option<Topology>, DeployError> {
    let state_path = workspace_root.join(".wh").join("state.json");
    if !state_path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&state_path).map_err(DeployError::FileRead)?;
    let topology: Topology = serde_json::from_str(&content)
        .map_err(|e| DeployError::ApplyFailed(format!("corrupt state file: {e}")))?;
    Ok(Some(topology))
}

/// Destroy a deployed topology: stop all containers, clear state, and commit to git.
///
/// This bypasses the plan/commit typestate chain since destroy is a teardown path,
/// not a build path. The `.wh` file path is used to locate the workspace root.
///
/// If no state exists (no `.wh/state.json`), returns a no-op result.
/// Partial failures (e.g., one container fails to stop) are logged but do not
/// halt the destroy — matching the `provision_containers()` error handling pattern.
///
/// The git commit uses ADR-003 format:
/// `[agent-name] destroy: removed N agents, M streams`
#[tracing::instrument(skip_all, fields(wh_file = %wh_file_path.as_ref().display()))]
pub fn destroy(
    wh_file_path: impl AsRef<Path>,
    agent_name: Option<&str>,
) -> Result<DestroyResult, DeployError> {
    let wh_path = wh_file_path.as_ref();
    let workspace_root = wh_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let current_state = load_state(workspace_root)?;

    let topology = match current_state {
        Some(t) => t,
        None => {
            // No state file — nothing deployed, no-op
            return Ok(DestroyResult {
                destroyed: 0,
                streams_removed: 0,
            });
        }
    };

    if topology.agents.is_empty() && topology.streams.is_empty() {
        // State exists but is empty — no-op
        return Ok(DestroyResult {
            destroyed: 0,
            streams_removed: 0,
        });
    }

    let agent = agent_name.unwrap_or("operator");
    let mut destroyed_count: usize = 0;
    let streams_removed = topology.streams.len();

    // Stop and remove all agent containers
    for agent_def in &topology.agents {
        let name = podman::container_name(&topology.name, &agent_def.name);
        match podman::podman_stop(&name) {
            Ok(()) => {
                destroyed_count += 1;
                tracing::info!(agent = %agent_def.name, "agent container destroyed");
            }
            Err(e) => {
                tracing::error!(
                    agent = %agent_def.name,
                    error = %e,
                    "failed to stop agent container during destroy — continuing"
                );
                // Still count as destroyed (intent recorded in git)
                destroyed_count += 1;
            }
        }
    }

    // Write cleared state to .wh/state.json
    let wh_dir = workspace_root.join(".wh");
    std::fs::create_dir_all(&wh_dir).map_err(DeployError::FileRead)?;

    let empty_topology = Topology {
        api_version: topology.api_version.clone(),
        name: topology.name.clone(),
        agents: vec![],
        streams: vec![],
        guardrails: topology.guardrails.clone(),
    };
    let state_path = wh_dir.join("state.json");
    let state_json = serde_json::to_string_pretty(&empty_topology)
        .map_err(|e| DeployError::ApplyFailed(format!("failed to serialize state: {e}")))?;
    std::fs::write(&state_path, &state_json).map_err(DeployError::FileRead)?;

    // Git commit: ensure gitignore, stage, scan, commit
    gitignore::ensure_gitignore(workspace_root)?;

    let stage_result = run_git_checked(workspace_root, &["add", ".wh/"]);
    if let Err(e) = stage_result {
        let _ = std::fs::remove_file(&state_path);
        return Err(e);
    }

    // Pre-commit secrets scan
    let suspicious = gitignore::scan_staged_for_secrets(workspace_root)?;
    if !suspicious.is_empty() {
        let _ = run_git(workspace_root, &["reset", "HEAD"]);
        let _ = std::fs::remove_file(&state_path);
        return Err(DeployError::SecretsDetected(suspicious));
    }

    // Build destroy summary
    let mut summary_parts = Vec::new();
    if destroyed_count > 0 {
        summary_parts.push(format!(
            "removed {} agent{}",
            destroyed_count,
            if destroyed_count == 1 { "" } else { "s" }
        ));
    }
    if streams_removed > 0 {
        summary_parts.push(format!(
            "removed {} stream{}",
            streams_removed,
            if streams_removed == 1 { "" } else { "s" }
        ));
    }
    let summary = if summary_parts.is_empty() {
        "no changes".to_string()
    } else {
        summary_parts.join(", ")
    };

    let commit_message = format!("[{agent}] destroy: {summary}");
    let commit_result = run_git(workspace_root, &["commit", "-m", &commit_message])?;

    if !commit_result.status.success() {
        let stderr = String::from_utf8_lossy(&commit_result.stderr);
        return Err(DeployError::GitFailed(format!(
            "git commit failed: {stderr}"
        )));
    }

    Ok(DestroyResult {
        destroyed: destroyed_count,
        streams_removed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_summary_formats_correctly() {
        let plan = PlanOutput {
            has_changes: true,
            changes: vec![crate::deploy::Change {
                op: "~".to_string(),
                component: "agent researcher".to_string(),
                field: Some("replicas".to_string()),
                from: Some(serde_json::json!(1)),
                to: Some(serde_json::json!(2)),
            }],
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
            changes: vec![crate::deploy::Change {
                op: "+".to_string(),
                component: "agent donna".to_string(),
                field: None,
                from: None,
                to: None,
            }],
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
