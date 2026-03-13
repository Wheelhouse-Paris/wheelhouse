//! Podman container lifecycle management for agent provisioning.
//!
//! Provides functions to start, stop, and check Podman containers
//! for agents declared in a Wheelhouse topology.
//! Podman is the only container provider for MVP (Docker explicitly excluded).

use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::deploy::{Change, DeployError};

/// Timeout for `podman run` (image pull may be slow on first run).
const PODMAN_RUN_TIMEOUT: Duration = Duration::from_secs(120);

/// Timeout for `podman stop`, `podman rm`, `podman ps` commands.
const PODMAN_CMD_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for `podman machine start`.
const PODMAN_MACHINE_START_TIMEOUT: Duration = Duration::from_secs(90);

/// Default broker endpoint for agent containers on Linux.
const DEFAULT_BROKER_URL: &str = "tcp://127.0.0.1:5555";

/// Broker endpoint for agent containers on macOS + Podman.
///
/// On macOS, Podman runs containers in a Linux VM. `127.0.0.1` inside the
/// container is the VM loopback, not the macOS host. `host.containers.internal`
/// is a special hostname Podman resolves to the macOS host gateway (192.168.127.254),
/// which reaches the broker bound on `0.0.0.0` on the macOS host.
#[cfg(target_os = "macos")]
const CONTAINER_BROKER_URL: &str = "tcp://host.containers.internal:5555";
#[cfg(not(target_os = "macos"))]
const CONTAINER_BROKER_URL: &str = DEFAULT_BROKER_URL;

/// TCP address used to probe whether the broker control socket is reachable.
/// Port 5557 = control REP socket (matches DEFAULT_CONTROL_PORT in config.rs).
const BROKER_CONTROL_ADDR: &str = "127.0.0.1:5557";

/// Maximum time to wait for the broker to start after spawning it.
const BROKER_START_TIMEOUT: Duration = Duration::from_secs(5);

/// Polling interval while waiting for the broker to start.
const BROKER_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Result of applying a set of changes to the container infrastructure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyResult {
    /// Number of containers created.
    pub created: usize,
    /// Number of containers changed (stopped + restarted).
    pub changed: usize,
    /// Number of containers destroyed.
    pub destroyed: usize,
    /// Number of streams registered (broker-managed, no container operation).
    pub streams_created: usize,
}

impl std::fmt::Display for ApplyResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} created \u{00B7} {} changed \u{00B7} {} destroyed \u{00B7} {} streams",
            self.created, self.changed, self.destroyed, self.streams_created
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
    let output = Command::new("which").arg("podman").output();
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

/// Ensure the Podman machine is running, starting it if necessary.
///
/// Runs `podman info` to check connectivity. If it fails (machine not started),
/// attempts `podman machine start` and waits for it to come up.
/// Returns an error if the machine cannot be started (e.g., not initialized).
pub fn ensure_podman_running() -> Result<(), DeployError> {
    let podman = find_podman()?;

    // Check if Podman is already reachable
    let info = run_podman(podman, &["info"], PODMAN_CMD_TIMEOUT);
    if info.is_ok_and(|o| o.status.success()) {
        return Ok(());
    }

    // Not reachable — try to start the machine
    tracing::info!("Podman machine not running — attempting `podman machine start`");
    eprintln!("  Starting Podman machine...");

    let start_output = run_podman(podman, &["machine", "start"], PODMAN_MACHINE_START_TIMEOUT)?;
    if !start_output.status.success() {
        let stderr = String::from_utf8_lossy(&start_output.stderr);
        return Err(DeployError::PodmanFailed(format!(
            "podman machine start failed: {stderr}\nRun `podman machine init` first if no machine exists."
        )));
    }

    // Verify it's now reachable (give it a moment to initialize)
    std::thread::sleep(Duration::from_secs(2));
    let check = run_podman(podman, &["info"], PODMAN_CMD_TIMEOUT)?;
    if !check.status.success() {
        return Err(DeployError::PodmanFailed(
            "Podman machine started but is still not reachable. Try running `podman info` manually.".to_string(),
        ));
    }

    eprintln!("  Podman machine started.");
    Ok(())
}

/// Check if the broker control socket is reachable via a TCP connection.
fn is_broker_reachable() -> bool {
    let addr: std::net::SocketAddr = BROKER_CONTROL_ADDR
        .parse()
        .expect("hardcoded address is valid");
    TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok()
}

/// Find the wh-broker binary: same directory as the current executable, or PATH.
fn find_broker_binary() -> Option<PathBuf> {
    // Check the same directory as the currently running wh binary first
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("wh-broker");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    // Fall back to PATH
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            let candidate = Path::new(dir).join("wh-broker");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Ensure the Wheelhouse broker is running, starting it automatically if not.
///
/// Probes the broker control socket via TCP. If unreachable, spawns `wh-broker`
/// as a detached background process (stdin/stdout/stderr to /dev/null) and polls
/// until it becomes reachable (up to 5 seconds).
///
/// This mirrors `ensure_podman_running()` — the broker is infrastructure, not a
/// user-managed object, and must be running before agent containers start.
pub fn ensure_broker_running() -> Result<(), DeployError> {
    if is_broker_reachable() {
        return Ok(());
    }

    let broker_bin = find_broker_binary().ok_or_else(|| {
        DeployError::ApplyFailed(
            "wh-broker not found. Ensure it is installed alongside the wh CLI.".to_string(),
        )
    })?;

    tracing::info!(binary = %broker_bin.display(), "broker not running — spawning wh-broker");
    eprintln!("  Starting Wheelhouse broker...");

    Command::new(&broker_bin)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| DeployError::ApplyFailed(format!("failed to spawn wh-broker: {e}")))?;

    // Poll until reachable or timeout
    let start = std::time::Instant::now();
    loop {
        if is_broker_reachable() {
            eprintln!("  Wheelhouse broker started.");
            return Ok(());
        }
        if start.elapsed() > BROKER_START_TIMEOUT {
            return Err(DeployError::ApplyFailed(
                "wh-broker did not start in time. Run `wh broker logs` to debug.".to_string(),
            ));
        }
        std::thread::sleep(BROKER_POLL_INTERVAL);
    }
}

/// Sanitize a string for use in a container name.
///
/// Replaces non-alphanumeric chars (except `-`) with `-`,
/// collapses multiple `-`, and trims leading/trailing `-`.
fn sanitize_name(name: &str) -> String {
    let mut result: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
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
                return child
                    .wait_with_output()
                    .map_err(|e| DeployError::PodmanFailed(format!("podman process error: {e}")));
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
                return Err(DeployError::PodmanFailed(format!(
                    "podman process error: {e}"
                )));
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
/// When `persona_path` is provided, adds a read-only volume mount and
/// `WH_PERSONA_PATH` environment variable for persona files.
/// `extra_env` is a list of additional `(KEY, VALUE)` pairs injected as `-e` flags
/// (used to pass secrets like `ANTHROPIC_API_KEY` from the CLI keychain).
pub fn build_run_args(
    topology_name: &str,
    agent_name: &str,
    image: &str,
    streams: &[String],
    broker_url: Option<&str>,
    persona_path: Option<&str>,
    extra_env: &[(String, String)],
) -> Vec<String> {
    let name = container_name(topology_name, agent_name);
    let url = broker_url.unwrap_or(DEFAULT_BROKER_URL);
    let streams_csv = streams.join(",");

    let mut args = vec![
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
    ];

    // Inject caller-provided secrets/env vars (e.g. ANTHROPIC_API_KEY)
    for (key, value) in extra_env {
        args.push("-e".to_string());
        args.push(format!("{key}={value}"));
    }

    // Add persona volume mount and env var when configured
    if let Some(path) = persona_path {
        args.push("-v".to_string());
        args.push(format!("{path}:/persona:ro"));
        args.push("-e".to_string());
        args.push("WH_PERSONA_PATH=/persona".to_string());
    }

    args.push(image.to_string());
    args
}

/// Start an agent container via Podman.
///
/// Uses `podman run -d` with the appropriate environment variables.
/// When `persona_path` is provided, mounts persona files read-only.
/// `extra_env` is forwarded to `build_run_args` for secret injection.
/// Timeout: 120s (image pull may be slow on first run).
#[tracing::instrument(skip_all, fields(agent = agent_name, topology = topology_name))]
pub fn podman_run(
    topology_name: &str,
    agent_name: &str,
    image: &str,
    streams: &[String],
    broker_url: Option<&str>,
    persona_path: Option<&str>,
    extra_env: &[(String, String)],
) -> Result<(), DeployError> {
    let podman = find_podman()?;
    let args = build_run_args(
        topology_name,
        agent_name,
        image,
        streams,
        broker_url,
        persona_path,
        extra_env,
    );
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    tracing::info!("starting agent container");
    run_podman_checked(podman, &args_ref, PODMAN_RUN_TIMEOUT)?;
    tracing::info!("agent container started");
    Ok(())
}

/// Build command arguments for `podman stop`.
///
/// Returns the argument list for stopping a container.
pub fn build_stop_args(container_name: &str) -> Vec<String> {
    vec!["stop".to_string(), container_name.to_string()]
}

/// Build command arguments for `podman rm -f`.
///
/// Returns the argument list for force-removing a container.
pub fn build_rm_args(container_name: &str) -> Vec<String> {
    vec![
        "rm".to_string(),
        "-f".to_string(),
        container_name.to_string(),
    ]
}

/// Build command arguments for `podman ps --filter`.
///
/// Returns the argument list for checking if a container is running.
pub fn build_ps_args(container_name: &str) -> Vec<String> {
    vec![
        "ps".to_string(),
        "--filter".to_string(),
        format!("name={container_name}"),
        "--format".to_string(),
        "{{.Status}}".to_string(),
    ]
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
        &[
            "ps",
            "--filter",
            &format!("name={name}"),
            "--format",
            "{{.Status}}",
        ],
        PODMAN_CMD_TIMEOUT,
    )?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(!stdout.trim().is_empty())
}

/// Parse a component string like "agent researcher" to extract the agent name.
fn parse_agent_name(component: &str) -> Option<&str> {
    component.strip_prefix("agent ")
}

/// Resolve an agent's persona path to an absolute path for volume mounting.
///
/// Returns `None` if the agent has no persona configured or if workspace_root
/// is not available.
fn resolve_persona_path(
    agent: &crate::deploy::Agent,
    workspace_root: Option<&std::path::Path>,
) -> Option<String> {
    match (&agent.persona, workspace_root) {
        (Some(persona_rel), Some(ws_root)) => {
            let abs_path = ws_root.join(persona_rel);
            Some(abs_path.to_string_lossy().to_string())
        }
        _ => None,
    }
}

/// Provision containers based on plan changes.
///
/// Iterates over changes and starts/stops containers as needed.
/// Stream changes are no-ops (local provider, handled by broker).
/// Returns an `ApplyResult` with change counts.
///
/// `extra_env` is injected into every container as additional `-e` flags.
/// Used to pass secrets (e.g. `ANTHROPIC_API_KEY`) read from the CLI keychain.
///
/// When an agent has a `persona` path configured, persona files are
/// validated before container startup and MEMORY.md is initialized
/// if missing (FR61). The persona directory is volume-mounted read-only.
///
/// On container failure, logs the error but does NOT fail the entire apply.
/// The git commit is already done; the operator can retry `apply` (idempotent).
#[tracing::instrument(skip_all)]
pub fn provision_containers(
    topology_name: &str,
    changes: &[Change],
    agents: &[crate::deploy::Agent],
    workspace_root: Option<&std::path::Path>,
    extra_env: &[(String, String)],
) -> ApplyResult {
    // Count stream additions upfront — streams require no container operation.
    let streams_created = changes
        .iter()
        .filter(|c| parse_agent_name(&c.component).is_none() && c.op == "+")
        .count();

    // Ensure Podman is running before attempting any container operations.
    // Starts the machine automatically if it is stopped.
    if let Err(e) = ensure_podman_running() {
        tracing::error!(error = %e, "Podman is not available");
        eprintln!("Error: {e}");
        return ApplyResult {
            created: 0,
            changed: 0,
            destroyed: 0,
            streams_created,
        };
    }

    // Ensure the broker is running before starting agent containers.
    // Agents connect to the broker at startup, so it must be available first.
    if let Err(e) = ensure_broker_running() {
        tracing::error!(error = %e, "broker is not available");
        eprintln!("Error: {e}");
        return ApplyResult {
            created: 0,
            changed: 0,
            destroyed: 0,
            streams_created,
        };
    }

    let mut result = ApplyResult {
        created: 0,
        changed: 0,
        destroyed: 0,
        streams_created,
    };

    for change in changes {
        // Skip stream changes — already counted above.
        if parse_agent_name(&change.component).is_none() {
            continue;
        }
        // Agent changes — handle container lifecycle.
        let agent_name = match parse_agent_name(&change.component) {
            Some(name) => name,
            None => continue,
        };

        match change.op.as_str() {
            "+" => {
                // Find the agent in the topology to get its image and streams
                let Some(agent) = agents.iter().find(|a| a.name == agent_name) else {
                    tracing::warn!(
                        agent = %agent_name,
                        "agent not found in topology — skipping container creation"
                    );
                    continue;
                };

                // Resolve persona path for volume mount (FR61)
                let persona_abs = resolve_persona_path(agent, workspace_root);

                // Validate persona files before starting container (FR61)
                // SOUL.md and IDENTITY.md are required — fail if missing
                if let Some(ref persona_rel) = agent.persona {
                    if let Some(ws_root) = workspace_root {
                        if let Err(e) = crate::deploy::persona::load_persona(ws_root, persona_rel) {
                            tracing::error!(
                                agent = %agent.name,
                                error = %e,
                                "persona validation failed — skipping container creation"
                            );
                            continue;
                        }
                    }
                }

                match podman_run(
                    topology_name,
                    &agent.name,
                    &agent.image,
                    &agent.streams,
                    Some(CONTAINER_BROKER_URL),
                    persona_abs.as_deref(),
                    extra_env,
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
                let Some(agent) = agents.iter().find(|a| a.name == agent_name) else {
                    tracing::warn!(
                        agent = %agent_name,
                        "agent not found in topology — skipping container restart"
                    );
                    continue;
                };
                // Resolve persona path for volume mount (FR61)
                let persona_abs = resolve_persona_path(agent, workspace_root);

                // Validate persona files before restarting container (FR61)
                // SOUL.md and IDENTITY.md are required — fail if missing
                if let Some(ref persona_rel) = agent.persona {
                    if let Some(ws_root) = workspace_root {
                        if let Err(e) = crate::deploy::persona::load_persona(ws_root, persona_rel) {
                            tracing::error!(
                                agent = %agent.name,
                                error = %e,
                                "persona validation failed — skipping container restart"
                            );
                            continue;
                        }
                    }
                }

                // Stop old
                let _ = podman_stop(&name);
                // Start new
                match podman_run(
                    topology_name,
                    &agent.name,
                    &agent.image,
                    &agent.streams,
                    Some(CONTAINER_BROKER_URL),
                    persona_abs.as_deref(),
                    extra_env,
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
        let args = build_run_args(
            "dev",
            "researcher",
            "researcher:latest",
            &["main".to_string()],
            None,
            None,
            &[],
        );
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
        let args = build_run_args(
            "dev",
            "donna",
            "donna:v1",
            &["events".to_string(), "logs".to_string()],
            Some("tcp://10.0.0.1:5555"),
            None,
            &[],
        );
        assert_eq!(args[5], "WH_URL=tcp://10.0.0.1:5555");
        assert_eq!(args[9], "WH_STREAMS=events,logs");
    }

    #[test]
    fn build_run_args_with_persona() {
        let args = build_run_args(
            "dev",
            "donna",
            "agent:latest",
            &["main".to_string()],
            None,
            Some("/workspace/agents/donna"),
            &[],
        );
        // Should contain volume mount
        let v_idx = args
            .iter()
            .position(|a| a == "-v")
            .expect("should have -v flag");
        assert!(
            args[v_idx + 1].contains("/persona:ro"),
            "volume mount should map to /persona:ro"
        );
        // Should contain WH_PERSONA_PATH env
        assert!(
            args.iter().any(|a| a == "WH_PERSONA_PATH=/persona"),
            "should have WH_PERSONA_PATH env var"
        );
    }

    #[test]
    fn build_run_args_without_persona_has_no_persona_args() {
        let args = build_run_args(
            "dev",
            "researcher",
            "r:latest",
            &["main".to_string()],
            None,
            None,
            &[],
        );
        assert!(
            !args
                .iter()
                .any(|a| a.contains("persona") || a.contains("PERSONA")),
            "should not have persona args"
        );
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
        let changes = vec![Change {
            op: "+".to_string(),
            component: "stream main".to_string(),
            field: None,
            from: None,
            to: None,
        }];
        let result = provision_containers("dev", &changes, &[], None, &[]);
        assert_eq!(result.created, 0);
        assert_eq!(result.changed, 0);
        assert_eq!(result.destroyed, 0);
        assert_eq!(result.streams_created, 1);
    }

    #[test]
    fn apply_result_display() {
        let result = ApplyResult {
            created: 1,
            changed: 0,
            destroyed: 2,
            streams_created: 1,
        };
        assert_eq!(
            result.to_string(),
            "1 created \u{00B7} 0 changed \u{00B7} 2 destroyed \u{00B7} 1 streams"
        );
    }

    #[test]
    fn build_stop_args_correct() {
        let args = build_stop_args("wh-dev-researcher");
        assert_eq!(args, vec!["stop", "wh-dev-researcher"]);
    }

    #[test]
    fn build_rm_args_correct() {
        let args = build_rm_args("wh-dev-researcher");
        assert_eq!(args, vec!["rm", "-f", "wh-dev-researcher"]);
    }

    #[test]
    fn build_ps_args_correct() {
        let args = build_ps_args("wh-dev-researcher");
        assert_eq!(args[0], "ps");
        assert_eq!(args[1], "--filter");
        assert_eq!(args[2], "name=wh-dev-researcher");
        assert_eq!(args[3], "--format");
        assert_eq!(args[4], "{{.Status}}");
    }

    #[test]
    fn provision_containers_skips_unknown_agent() {
        // Agent "ghost" is in the change list but not in the agents vec
        let changes = vec![Change {
            op: "+".to_string(),
            component: "agent ghost".to_string(),
            field: None,
            from: None,
            to: None,
        }];
        let result = provision_containers("dev", &changes, &[], None, &[]);
        // Should skip gracefully, not panic, and not increment created
        assert_eq!(result.created, 0);
    }
}
