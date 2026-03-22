//! Built-in `wh-cli` skill handler (ADR-035).
//!
//! Intercepts `SkillInvocation` with `skill_name == "wh-cli"` and executes
//! the requested `wh` command on the host. Output is streamed line-by-line
//! via `SkillProgress` messages, followed by a terminal `SkillResult`.
//!
//! Two-tier permissions (FR77):
//! - Tier 1 (read-only): any agent with `wh-cli` skill
//! - Tier 2 (write): requires `topology_edit: true`
//! - Blocked: `secrets`, `completion`, `doctor` — always rejected (E12-23)
//!
//! Constraints: E12-19 (built-in, not git-loaded), E12-20 (hardcoded allowlist),
//! E12-21 (per-line SkillProgress), E12-22 (60s timeout), E12-23 (secrets blocked).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use prost::Message;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use wh_skill::invocation::{
    build_skill_progress_chunk, build_skill_result_error, build_skill_result_success,
};

use crate::skill_router::{SkillResponse, TYPE_URL_SKILL_PROGRESS, TYPE_URL_SKILL_RESULT};

/// The well-known skill name for the built-in CLI handler.
pub const WH_CLI_SKILL_NAME: &str = "wh-cli";

/// Command execution timeout (E12-22: 60 seconds, non-configurable in MVP).
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

/// Command permission tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandTier {
    /// Read-only commands — any agent with `wh-cli` skill can execute.
    Tier1,
    /// Write commands — requires `topology_edit: true`.
    Tier2,
    /// Unconditionally blocked commands (E12-23).
    Blocked,
}

/// Classify a command by its first token(s) into a permission tier.
///
/// Multi-word subcommands (e.g., `topology plan`, `stream tail`) are checked
/// by examining the first two tokens. Single-word commands are checked by the
/// first token alone.
///
/// Returns `Blocked` for unknown commands (E12-20: hardcoded allowlist).
pub fn classify_command(args: &[&str]) -> CommandTier {
    if args.is_empty() {
        return CommandTier::Blocked;
    }

    let first = args[0];
    let second = args.get(1).copied().unwrap_or("");

    // Blocked commands — unconditionally rejected (E12-23)
    match first {
        "secrets" | "completion" | "doctor" => return CommandTier::Blocked,
        _ => {}
    }

    // Two-token subcommands
    match (first, second) {
        // Tier 1 — read-only
        ("topology", "plan") | ("topology", "lint") => return CommandTier::Tier1,
        ("stream", "tail") => return CommandTier::Tier1,
        // Tier 2 — write (requires topology_edit)
        ("topology", "apply") | ("topology", "destroy") => return CommandTier::Tier2,
        ("skill", "create") => return CommandTier::Tier2,
        _ => {}
    }

    // Single-token Tier 1 commands
    match first {
        "ps" | "status" | "logs" | "capabilities" | "reference" => CommandTier::Tier1,
        // Unknown command — blocked (E12-20: hardcoded allowlist)
        _ => CommandTier::Blocked,
    }
}

/// Built-in `wh-cli` skill handler.
///
/// Stores the topology folder path (used as working directory for child processes)
/// and agent permission map (agent_id -> topology_edit).
pub struct WhCliHandler {
    /// Working directory for `wh` child processes (FR79).
    topology_dir: PathBuf,
    /// Map of agent_id -> topology_edit permission.
    /// Agents not in this map default to `false`.
    agent_permissions: HashMap<String, bool>,
}

impl WhCliHandler {
    /// Create a new handler with the given topology directory and agent permissions.
    pub fn new(topology_dir: PathBuf, agent_permissions: HashMap<String, bool>) -> Self {
        Self {
            topology_dir,
            agent_permissions,
        }
    }

    /// Check if the given agent has `topology_edit` permission.
    fn has_topology_edit(&self, agent_id: &str) -> bool {
        self.agent_permissions
            .get(agent_id)
            .copied()
            .unwrap_or(false)
    }

    /// Execute a `wh-cli` skill invocation.
    ///
    /// Returns a list of `SkillResponse` messages: zero or more `SkillProgress`
    /// followed by exactly one terminal `SkillResult`.
    pub async fn execute(
        &self,
        agent_id: &str,
        invocation_id: &str,
        args_str: &str,
    ) -> Vec<SkillResponse> {
        let args: Vec<&str> = args_str.split_whitespace().collect();

        // Classify the command
        let tier = classify_command(&args);

        // Check permissions
        match tier {
            CommandTier::Blocked => {
                let first_cmd = args.first().copied().unwrap_or("(empty)");
                tracing::warn!(
                    agent_id = agent_id,
                    command = first_cmd,
                    "wh-cli: blocked command rejected (E12-23)"
                );
                let result = build_skill_result_error(
                    invocation_id,
                    WH_CLI_SKILL_NAME,
                    "WH_CLI_COMMAND_BLOCKED",
                    &format!("command '{first_cmd}' is blocked"),
                );
                return vec![SkillResponse {
                    type_url: TYPE_URL_SKILL_RESULT.to_string(),
                    payload: result.encode_to_vec(),
                }];
            }
            CommandTier::Tier2 => {
                if !self.has_topology_edit(agent_id) {
                    tracing::warn!(
                        agent_id = agent_id,
                        args = args_str,
                        "wh-cli: tier-2 command denied — agent requires topology_edit capability"
                    );
                    let result = build_skill_result_error(
                        invocation_id,
                        WH_CLI_SKILL_NAME,
                        "WH_CLI_PERMISSION_DENIED",
                        &format!("command '{args_str}' requires topology_edit capability"),
                    );
                    return vec![SkillResponse {
                        type_url: TYPE_URL_SKILL_RESULT.to_string(),
                        payload: result.encode_to_vec(),
                    }];
                }
            }
            CommandTier::Tier1 => {
                // Allowed for any agent with wh-cli skill
            }
        }

        // Execute the command
        self.run_command(invocation_id, &args).await
    }

    /// Spawn `wh <args>` and stream output line-by-line as `SkillProgress`.
    async fn run_command(&self, invocation_id: &str, args: &[&str]) -> Vec<SkillResponse> {
        let mut responses = Vec::new();

        let child_result = Command::new("wh")
            .args(args)
            .current_dir(&self.topology_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn();

        let mut child = match child_result {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "wh-cli: failed to spawn `wh` child process"
                );
                let result = build_skill_result_error(
                    invocation_id,
                    WH_CLI_SKILL_NAME,
                    "WH_CLI_SPAWN_FAILED",
                    &format!("failed to spawn `wh` process: {e}"),
                );
                return vec![SkillResponse {
                    type_url: TYPE_URL_SKILL_RESULT.to_string(),
                    payload: result.encode_to_vec(),
                }];
            }
        };

        // Take stdout and stderr handles
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // Read stdout and stderr concurrently, merge lines.
        // Each line is published as a SkillProgress with incrementing sequence (E12-21).
        let read_future = async {
            let mut output_lines: Vec<String> = Vec::new();
            let mut sequence: u32 = 0;

            if let Some(stdout) = stdout {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    // Send SkillProgress per line (E12-21, Story 12-7)
                    let progress =
                        build_skill_progress_chunk(invocation_id, WH_CLI_SKILL_NAME, &line, sequence);
                    responses.push(SkillResponse {
                        type_url: TYPE_URL_SKILL_PROGRESS.to_string(),
                        payload: progress.encode_to_vec(),
                    });
                    output_lines.push(line);
                    sequence += 1;
                }
            }

            // Also read stderr lines
            if let Some(stderr) = stderr {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let progress =
                        build_skill_progress_chunk(invocation_id, WH_CLI_SKILL_NAME, &line, sequence);
                    responses.push(SkillResponse {
                        type_url: TYPE_URL_SKILL_PROGRESS.to_string(),
                        payload: progress.encode_to_vec(),
                    });
                    output_lines.push(line);
                    sequence += 1;
                }
            }

            output_lines
        };

        // Apply 60-second timeout (E12-22)
        match tokio::time::timeout(COMMAND_TIMEOUT, async {
            let output_lines = read_future.await;
            let status = child.wait().await;
            (output_lines, status)
        })
        .await
        {
            Ok((output_lines, Ok(status))) => {
                let full_output = output_lines.join("\n");
                let exit_code = status.code().unwrap_or(-1);

                if status.success() {
                    let result =
                        build_skill_result_success(invocation_id, WH_CLI_SKILL_NAME, &full_output);
                    responses.push(SkillResponse {
                        type_url: TYPE_URL_SKILL_RESULT.to_string(),
                        payload: result.encode_to_vec(),
                    });
                } else {
                    let result = build_skill_result_error(
                        invocation_id,
                        WH_CLI_SKILL_NAME,
                        "WH_CLI_EXIT_ERROR",
                        &format!("command exited with code {exit_code}: {full_output}"),
                    );
                    responses.push(SkillResponse {
                        type_url: TYPE_URL_SKILL_RESULT.to_string(),
                        payload: result.encode_to_vec(),
                    });
                }
            }
            Ok((output_lines, Err(e))) => {
                let full_output = output_lines.join("\n");
                let result = build_skill_result_error(
                    invocation_id,
                    WH_CLI_SKILL_NAME,
                    "WH_CLI_EXECUTION_FAILED",
                    &format!("command failed: {e}. Output: {full_output}"),
                );
                responses.push(SkillResponse {
                    type_url: TYPE_URL_SKILL_RESULT.to_string(),
                    payload: result.encode_to_vec(),
                });
            }
            Err(_) => {
                // Timeout — kill the child process (E12-22)
                tracing::warn!(
                    invocation_id = invocation_id,
                    "wh-cli: command timed out after 60 seconds — killing child process"
                );
                let _ = child.kill().await;
                let result = build_skill_result_error(
                    invocation_id,
                    WH_CLI_SKILL_NAME,
                    "WH_CLI_TIMEOUT",
                    "command timed out after 60 seconds",
                );
                responses.push(SkillResponse {
                    type_url: TYPE_URL_SKILL_RESULT.to_string(),
                    payload: result.encode_to_vec(),
                });
            }
        }

        responses
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_tier1_single_word_commands() {
        assert_eq!(classify_command(&["ps"]), CommandTier::Tier1);
        assert_eq!(classify_command(&["status"]), CommandTier::Tier1);
        assert_eq!(classify_command(&["logs"]), CommandTier::Tier1);
        assert_eq!(classify_command(&["capabilities"]), CommandTier::Tier1);
        assert_eq!(classify_command(&["reference"]), CommandTier::Tier1);
    }

    #[test]
    fn classify_tier1_two_word_commands() {
        assert_eq!(classify_command(&["topology", "plan"]), CommandTier::Tier1);
        assert_eq!(classify_command(&["topology", "lint"]), CommandTier::Tier1);
        assert_eq!(classify_command(&["stream", "tail"]), CommandTier::Tier1);
    }

    #[test]
    fn classify_tier1_with_extra_args() {
        assert_eq!(
            classify_command(&["topology", "plan", "--format", "json"]),
            CommandTier::Tier1
        );
        assert_eq!(
            classify_command(&["stream", "tail", "--last", "10"]),
            CommandTier::Tier1
        );
        assert_eq!(classify_command(&["logs", "agent-1"]), CommandTier::Tier1);
    }

    #[test]
    fn classify_tier2_commands() {
        assert_eq!(classify_command(&["topology", "apply"]), CommandTier::Tier2);
        assert_eq!(
            classify_command(&["topology", "apply", "--yes"]),
            CommandTier::Tier2
        );
        assert_eq!(
            classify_command(&["topology", "destroy"]),
            CommandTier::Tier2
        );
        assert_eq!(classify_command(&["skill", "create"]), CommandTier::Tier2);
    }

    #[test]
    fn classify_blocked_commands() {
        assert_eq!(classify_command(&["secrets"]), CommandTier::Blocked);
        assert_eq!(classify_command(&["secrets", "list"]), CommandTier::Blocked);
        assert_eq!(classify_command(&["completion"]), CommandTier::Blocked);
        assert_eq!(
            classify_command(&["completion", "bash"]),
            CommandTier::Blocked
        );
        assert_eq!(classify_command(&["doctor"]), CommandTier::Blocked);
    }

    #[test]
    fn classify_unknown_command_blocked() {
        assert_eq!(
            classify_command(&["nonexistent-command"]),
            CommandTier::Blocked
        );
        assert_eq!(classify_command(&["deploy"]), CommandTier::Blocked);
        assert_eq!(classify_command(&["init"]), CommandTier::Blocked);
    }

    #[test]
    fn classify_empty_args_blocked() {
        assert_eq!(classify_command(&[]), CommandTier::Blocked);
    }

    #[test]
    fn handler_topology_edit_lookup() {
        let mut perms = HashMap::new();
        perms.insert("donna".to_string(), true);
        perms.insert("researcher".to_string(), false);

        let handler = WhCliHandler::new(PathBuf::from("/tmp"), perms);

        assert!(handler.has_topology_edit("donna"));
        assert!(!handler.has_topology_edit("researcher"));
        assert!(!handler.has_topology_edit("unknown-agent"));
    }

    #[tokio::test]
    async fn execute_blocked_command_returns_error() {
        let handler = WhCliHandler::new(PathBuf::from("/tmp"), HashMap::new());

        let responses = handler.execute("agent-1", "inv-001", "secrets list").await;
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].type_url, TYPE_URL_SKILL_RESULT);

        let result = wh_proto::SkillResult::decode(responses[0].payload.as_slice()).unwrap();
        assert!(!result.success);
        assert_eq!(result.error_code, "WH_CLI_COMMAND_BLOCKED");
        assert!(result.error_message.contains("blocked"));
    }

    #[tokio::test]
    async fn execute_tier2_denied_without_topology_edit() {
        let handler = WhCliHandler::new(PathBuf::from("/tmp"), HashMap::new());

        let responses = handler
            .execute("researcher", "inv-002", "topology apply --yes")
            .await;
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].type_url, TYPE_URL_SKILL_RESULT);

        let result = wh_proto::SkillResult::decode(responses[0].payload.as_slice()).unwrap();
        assert!(!result.success);
        assert_eq!(result.error_code, "WH_CLI_PERMISSION_DENIED");
        assert!(result.error_message.contains("topology_edit"));
    }

    #[tokio::test]
    async fn execute_blocked_even_with_topology_edit() {
        let mut perms = HashMap::new();
        perms.insert("donna".to_string(), true);
        let handler = WhCliHandler::new(PathBuf::from("/tmp"), perms);

        let responses = handler.execute("donna", "inv-003", "secrets list").await;
        assert_eq!(responses.len(), 1);

        let result = wh_proto::SkillResult::decode(responses[0].payload.as_slice()).unwrap();
        assert!(!result.success);
        assert_eq!(result.error_code, "WH_CLI_COMMAND_BLOCKED");
    }

    #[tokio::test]
    async fn execute_unknown_command_blocked() {
        let handler = WhCliHandler::new(PathBuf::from("/tmp"), HashMap::new());

        let responses = handler
            .execute("agent-1", "inv-004", "nonexistent-command")
            .await;
        assert_eq!(responses.len(), 1);

        let result = wh_proto::SkillResult::decode(responses[0].payload.as_slice()).unwrap();
        assert!(!result.success);
        assert_eq!(result.error_code, "WH_CLI_COMMAND_BLOCKED");
    }

    #[tokio::test]
    async fn execute_empty_args_blocked() {
        let handler = WhCliHandler::new(PathBuf::from("/tmp"), HashMap::new());

        let responses = handler.execute("agent-1", "inv-005", "").await;
        assert_eq!(responses.len(), 1);

        let result = wh_proto::SkillResult::decode(responses[0].payload.as_slice()).unwrap();
        assert!(!result.success);
        assert_eq!(result.error_code, "WH_CLI_COMMAND_BLOCKED");
    }
}
