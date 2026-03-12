//! Podman container lifecycle management for agent provisioning.
//!
//! Provides functions to start, stop, and check Podman containers
//! for agents declared in a Wheelhouse topology.
//! Podman is the only container provider for MVP (Docker explicitly excluded).

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::deploy::{Change, DeployError};

/// Timeout for `podman run` (image pull may be slow on first run).
const PODMAN_RUN_TIMEOUT: Duration = Duration::from_secs(120);

/// Timeout for `podman stop`, `podman rm`, `podman ps` commands.
const PODMAN_CMD_TIMEOUT: Duration = Duration::from_secs(30);

/// Default broker endpoint for agent containers.
const DEFAULT_BROKER_URL: &str = "tcp://127.0.0.1:5555";

/// Result of applying a set of changes to the container infrastructure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyResult {
    /// Number of containers created.
    pub created: usize,
    /// Number of containers changed (stopped + restarted).
    pub changed: usize,
    /// Number of containers destroyed.
    pub destroyed: usize,
}

impl std::fmt::Display for ApplyResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} created \u{00B7} {} changed \u{00B7} {} destroyed",
            self.created, self.changed, self.destroyed
        )
    }
}

/// Find the podman binary, checking common paths.
///
/// Returns the path to the podman binary, or an error if not found.
pub fn find_podman() -> Result<&'static str, DeployError> {
    for path in &[
        "/opt/podman/bin/podman",
        "/usr/bin/podman",
        "/usr/local/bin/podman",
        "/opt/homebrew/bin/podman",
    ] {
        if Path::new(path).exists() {
            return Ok(path);
        }
    }
    // Try PATH as fallback — check if podman is available
    let output = Command::new("which")
        .arg("podman")
        .output();
    if let Ok(out) = output {
        if out.status.success() {
            // podman is on PATH
            return Ok("podman");
        }
    }
    Err(DeployError::PodmanNotFound(
        "Podman is required but not found. Install from https://podman.io".to_string(),
    ))
}

/// Sanitize a string for use in a container name.
///
/// Replaces non-alphanumeric chars (except `-`) with `-`,
/// collapses multiple `-`, and trims leading/trailing `-`.
fn sanitize_name(name: &str) -> String {
    let mut result: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    // Collapse multiple dashes
    while result.contains("--") {
        result = result.replace("--", "-");
    }
    result.trim_matches('-').to_string()
}

/// Build the deterministic container name for an agent.
///
/// Format: `wh-<topology>-<agent>`
pub fn container_name(topology_name: &str, agent_name: &str) -> String {
    let topo = sanitize_name(topology_name);
    let agent = sanitize_name(agent_name);
    format!("wh-{topo}-{agent}")
}

/// Run a podman command with the given timeout.
fn run_podman(
    podman_bin: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::Output, DeployError> {
    let mut child = Command::new(podman_bin)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| DeployError::PodmanFailed(format!("failed to spawn podman: {e}")))?;

    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child.wait_with_output().map_err(|e| {
                    DeployError::PodmanFailed(format!("podman process error: {e}"))
                });
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(DeployError::PodmanFailed(format!(
                        "podman command timed out after {}s",
                        timeout.as_secs()
                    )));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                return Err(DeployError::PodmanFailed(format!("podman process error: {e}")));
            }
        }
    }
}

/// Run a podman command and check that it succeeded.
fn run_podman_checked(
    podman_bin: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::Output, DeployError> {
    let output = run_podman(podman_bin, args, timeout)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DeployError::PodmanFailed(format!(
            "podman {} failed: {stderr}",
            args.first().unwrap_or(&"")
        )));
    }
    Ok(output)
}

/// Build command arguments for `podman run`.
///
/// Returns the argument list (without the "podman" binary itself).
pub fn build_run_args(
    topology_name: &str,
    agent_name: &str,
    image: &str,
    streams: &[String],
    broker_url: Option<&str>,
) -> Vec<String> {
    let name = container_name(topology_name, agent_name);
    let url = broker_url.unwrap_or(DEFAULT_BROKER_URL);
    let streams_csv = streams.join(",");

    vec![
        "run".to_string(),
        "-d".to_string(),
        "--name".to_string(),
        name,
        "-e".to_string(),
        format!("WH_URL={url}"),
        "-e".to_string(),
        format!("WH_AGENT_NAME={agent_name}"),
        "-e".to_string(),
        format!("WH_STREAMS={streams_csv}"),
        image.to_string(),
    ]
}

/// Start an agent container via Podman.
///
/// Uses `podman run -d` with the appropriate environment variables.
/// Timeout: 120s (image pull may be slow on first run).
#[tracing::instrument(skip_all, fields(agent = agent_name, topology = topology_name))]
pub fn podman_run(
    topology_name: &str,
    agent_name: &str,
    image: &str,
    streams: &[String],
    broker_url: Option<&str>,
) -> Result<(), DeployError> {
    let podman = find_podman()?;
    let args = build_run_args(topology_name, agent_name, image, streams, broker_url);
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    tracing::info!("starting agent container");
    run_podman_checked(podman, &args_ref, PODMAN_RUN_TIMEOUT)?;
    tracing::info!("agent container started");
    Ok(())
}

/// Stop and remove an agent container.
///
/// Runs `podman stop` then `podman rm`. Ignores errors from stop
/// (container may already be stopped).
#[tracing::instrument(skip_all, fields(container = %name))]
pub fn podman_stop(name: &str) -> Result<(), DeployError> {
    let podman = find_podman()?;

    tracing::info!("stopping agent container");
    // Stop — ignore error (may already be stopped)
    let _ = run_podman(podman, &["stop", name], PODMAN_CMD_TIMEOUT);

    // Remove — this is the important part
    run_podman_checked(podman, &["rm", "-f", name], PODMAN_CMD_TIMEOUT)?;
    tracing::info!("agent container removed");
    Ok(())
}

/// Check if a container with the given name is currently running.
#[tracing::instrument(skip_all, fields(container = %name))]
pub fn podman_is_running(name: &str) -> Result<bool, DeployError> {
    let podman = find_podman()?;

    let output = run_podman(
        podman,
        &["ps", "--filter", &format!("name={name}"), "--format", "{{.Status}}"],
        PODMAN_CMD_TIMEOUT,
    )?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(!stdout.trim().is_empty())
}

/// Parse a component string like "agent researcher" to extract the agent name.
fn parse_agent_name(component: &str) -> Option<&str> {
    component.strip_prefix("agent ")
}

/// Provision containers based on plan changes.
///
/// Iterates over changes and starts/stops containers as needed.
/// Stream changes are no-ops (local provider, handled by broker).
/// Returns an `ApplyResult` with change counts.
///
/// On container failure, logs the error but does NOT fail the entire apply.
/// The git commit is already done; the operator can retry `apply` (idempotent).
#[tracing::instrument(skip_all)]
pub fn provision_containers(
    topology_name: &str,
    changes: &[Change],
    agents: &[crate::deploy::Agent],
) -> ApplyResult {
    let mut result = ApplyResult {
        created: 0,
        changed: 0,
        destroyed: 0,
    };

    for change in changes {
        // Only handle agent changes — stream changes are no-ops
        let agent_name = match parse_agent_name(&change.component) {
            Some(name) => name,
            None => continue, // skip stream changes
        };

        match change.op.as_str() {
            "+" => {
                // Find the agent in the topology to get its image and streams
                let agent = agents.iter().find(|a| a.name == agent_name);
                if let Some(agent) = agent {
                    match podman_run(
                        topology_name,
                        &agent.name,
                        &agent.image,
                        &agent.streams,
                        None,
                    ) {
                        Ok(()) => result.created += 1,
                        Err(e) => {
                            tracing::error!(
                                agent = %agent.name,
                                error = %e,
                                "failed to start agent container — retry with `wh deploy apply`"
                            );
                        }
                    }
                }
            }
            "-" => {
                let name = container_name(topology_name, agent_name);
                match podman_stop(&name) {
                    Ok(()) => result.destroyed += 1,
                    Err(e) => {
                        tracing::error!(
                            agent = %agent_name,
                            error = %e,
                            "failed to stop agent container — retry with `wh deploy apply`"
                        );
                    }
                }
            }
            "~" => {
                // For modifications, restart the container
                let name = container_name(topology_name, agent_name);
                let agent = agents.iter().find(|a| a.name == agent_name);
                if let Some(agent) = agent {
                    // Stop old
                    let _ = podman_stop(&name);
                    // Start new
                    match podman_run(
                        topology_name,
                        &agent.name,
                        &agent.image,
                        &agent.streams,
                        None,
                    ) {
                        Ok(()) => result.changed += 1,
                        Err(e) => {
                            tracing::error!(
                                agent = %agent.name,
                                error = %e,
                                "failed to restart agent container — retry with `wh deploy apply`"
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_name_formats_correctly() {
        assert_eq!(container_name("dev", "researcher"), "wh-dev-researcher");
        assert_eq!(container_name("my-app", "donna"), "wh-my-app-donna");
    }

    #[test]
    fn container_name_sanitizes_special_chars() {
        assert_eq!(container_name("my app", "agent.1"), "wh-my-app-agent-1");
        assert_eq!(container_name("dev", "--bad--"), "wh-dev-bad");
    }

    #[test]
    fn sanitize_name_works() {
        assert_eq!(sanitize_name("hello-world"), "hello-world");
        assert_eq!(sanitize_name("hello world"), "hello-world");
        assert_eq!(sanitize_name("--double--dash--"), "double-dash");
        assert_eq!(sanitize_name("abc123"), "abc123");
    }

    #[test]
    fn build_run_args_correct() {
        let args = build_run_args("dev", "researcher", "researcher:latest", &["main".to_string()], None);
        assert_eq!(args[0], "run");
        assert_eq!(args[1], "-d");
        assert_eq!(args[2], "--name");
        assert_eq!(args[3], "wh-dev-researcher");
        assert_eq!(args[4], "-e");
        assert_eq!(args[5], "WH_URL=tcp://127.0.0.1:5555");
        assert_eq!(args[6], "-e");
        assert_eq!(args[7], "WH_AGENT_NAME=researcher");
        assert_eq!(args[8], "-e");
        assert_eq!(args[9], "WH_STREAMS=main");
        assert_eq!(args[10], "researcher:latest");
    }

    #[test]
    fn build_run_args_custom_url() {
        let args = build_run_args("dev", "donna", "donna:v1", &["events".to_string(), "logs".to_string()], Some("tcp://10.0.0.1:5555"));
        assert_eq!(args[5], "WH_URL=tcp://10.0.0.1:5555");
        assert_eq!(args[9], "WH_STREAMS=events,logs");
    }

    #[test]
    fn parse_agent_name_works() {
        assert_eq!(parse_agent_name("agent researcher"), Some("researcher"));
        assert_eq!(parse_agent_name("agent donna"), Some("donna"));
        assert_eq!(parse_agent_name("stream main"), None);
        assert_eq!(parse_agent_name("something else"), None);
    }

    #[test]
    fn provision_containers_skips_stream_changes() {
        let changes = vec![
            Change {
                op: "+".to_string(),
                component: "stream main".to_string(),
                field: None,
                from: None,
                to: None,
            },
        ];
        let result = provision_containers("dev", &changes, &[]);
        assert_eq!(result.created, 0);
        assert_eq!(result.changed, 0);
        assert_eq!(result.destroyed, 0);
    }

    #[test]
    fn apply_result_display() {
        let result = ApplyResult { created: 1, changed: 0, destroyed: 2 };
        assert_eq!(result.to_string(), "1 created \u{00B7} 0 changed \u{00B7} 2 destroyed");
    }
}
