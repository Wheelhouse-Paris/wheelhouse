//! Podman container lifecycle management for agent provisioning.
//!
//! Provides functions to start, stop, and check Podman containers
//! for agents declared in a Wheelhouse topology.
//! Podman is the only container provider for MVP (Docker explicitly excluded).

use std::net::TcpStream;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::deploy::{Change, DeployError};

/// Timeout for `podman run` (image pull may be slow on first run).
const PODMAN_RUN_TIMEOUT: Duration = Duration::from_secs(120);

/// Timeout for `podman stop`, `podman rm`, `podman ps` commands.
const PODMAN_CMD_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for `podman machine start`.
const PODMAN_MACHINE_START_TIMEOUT: Duration = Duration::from_secs(90);

/// Broker endpoint for agent containers on the topology network (ADR-025).
///
/// All containers use DNS-based discovery within the Podman network.
/// The broker container is named `wh-broker` (ADR-024) and agents
/// connect via `tcp://wh-broker:5555`.
const BROKER_DNS_URL: &str = "tcp://wh-broker:5555";

/// Fixed container name for the broker (ADR-024).
///
/// No topology prefix — names are unique per network, not globally.
const BROKER_CONTAINER_NAME: &str = "wh-broker";

/// Default broker container image (ADR-028).
const BROKER_IMAGE: &str = "ghcr.io/wheelhouse-paris/wh-broker:latest";

/// TCP address used to probe whether the broker control socket is reachable
/// from the host via published port. Port 5557 = control REP socket.
const BROKER_CONTROL_ADDR: &str = "127.0.0.1:5557";

/// Broker endpoint for host-side processes (CLI).
///
/// The broker container publishes ports on `127.0.0.1`. The CLI surface
/// (the only native component) connects via this address. All other surfaces
/// run as containers and use `BROKER_DNS_URL` instead.
#[allow(dead_code)]
const HOST_BROKER_URL: &str = "tcp://127.0.0.1:5555";

/// Image prefix for surface containers (ADR-026).
///
/// Combined with the surface kind and tag to produce the full image name:
/// `ghcr.io/wheelhouse-paris/wh-<kind>:<tag>`
const SURFACE_IMAGE_PREFIX: &str = "ghcr.io/wheelhouse-paris/wh-";

/// Default image tag for surface containers.
const SURFACE_IMAGE_TAG: &str = "latest";

/// ZMQ endpoint for the broker control socket (host-side, published port).
///
/// Used by `register_stream_with_broker()` which runs on the host CLI,
/// not inside a container.
const BROKER_CONTROL_ENDPOINT: &str = "tcp://127.0.0.1:5557";

/// Maximum time to wait for the broker to start after spawning it.
const BROKER_START_TIMEOUT: Duration = Duration::from_secs(5);

/// Polling interval while waiting for the broker to start.
const BROKER_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Result of applying a set of changes to the container infrastructure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyResult {
    /// Number of agent containers created.
    pub created: usize,
    /// Number of agent containers changed (stopped + restarted).
    pub changed: usize,
    /// Number of agent containers destroyed.
    pub destroyed: usize,
    /// Number of streams registered (broker-managed, no container operation).
    pub streams_created: usize,
    /// Number of surface containers created.
    pub surfaces_created: usize,
    /// Number of surface containers changed (stopped + restarted).
    pub surfaces_changed: usize,
    /// Number of surface containers destroyed.
    pub surfaces_destroyed: usize,
}

impl std::fmt::Display for ApplyResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} created \u{00B7} {} changed \u{00B7} {} destroyed \u{00B7} {} streams \u{00B7} {} surfaces created \u{00B7} {} surfaces changed \u{00B7} {} surfaces destroyed",
            self.created, self.changed, self.destroyed, self.streams_created,
            self.surfaces_created, self.surfaces_changed, self.surfaces_destroyed
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

/// Ensure the broker is running as a Podman container on the topology network (ADR-025).
///
/// If a container named `wh-broker` is already running, this is a no-op (idempotent).
/// Otherwise, starts the broker container with:
/// - `--network wh-<topology>` for DNS-based discovery
/// - `-p 127.0.0.1:5555-5557:5555-5557` for host CLI access
/// - `-v wh-<topology>-wal:/data` for WAL persistence
/// - `-e WH_DATA_DIR=/data`
///
/// After starting, polls the control socket via TCP until reachable (up to 5s).
#[tracing::instrument(skip_all, fields(topology = %topology_name))]
pub fn ensure_broker_container(topology_name: &str) -> Result<(), DeployError> {
    // Idempotent: skip if already running.
    if podman_is_running(BROKER_CONTAINER_NAME)? {
        tracing::info!("broker container already running");
        return Ok(());
    }

    let podman = find_podman()?;
    let net_name = network_name(topology_name);
    let wal_volume = format!("wh-{}-wal", sanitize_name(topology_name));
    let volume_mount = format!("{wal_volume}:/data");

    let args = vec![
        "run",
        "-d",
        "--name",
        BROKER_CONTAINER_NAME,
        "--network",
        &net_name,
        "-p",
        "127.0.0.1:5555:5555",
        "-p",
        "127.0.0.1:5556:5556",
        "-p",
        "127.0.0.1:5557:5557",
        "-v",
        &volume_mount,
        "-e",
        "WH_DATA_DIR=/data",
        BROKER_IMAGE,
    ];

    tracing::info!("starting broker container");
    eprintln!("  Starting Wheelhouse broker container...");
    run_podman_checked(podman, &args, PODMAN_RUN_TIMEOUT)?;

    // Poll until the control socket is reachable from the host (published port).
    let start = std::time::Instant::now();
    loop {
        if is_broker_reachable() {
            tracing::info!("broker container ready");
            eprintln!("  Wheelhouse broker container started.");
            return Ok(());
        }
        if start.elapsed() > BROKER_START_TIMEOUT {
            return Err(DeployError::ApplyFailed(
                "broker container started but control socket not reachable. Run `podman logs wh-broker` to debug.".to_string(),
            ));
        }
        std::thread::sleep(BROKER_POLL_INTERVAL);
    }
}

/// Register a stream with the running broker via the control socket.
///
/// Sends a `stream_create` command over ZMQ REQ/REP. Errors are logged but not
/// propagated — stream registration is best-effort during deploy (the operator
/// can always run `wh stream create` manually if needed).
fn register_stream_with_broker(name: &str, retention: Option<&str>) {
    use zeromq::{ReqSocket, Socket, SocketRecv, SocketSend, ZmqMessage};

    let endpoint = BROKER_CONTROL_ENDPOINT.to_string();
    let name_owned = name.to_string();
    let retention_owned = retention.map(|r| r.to_string());

    // provision_containers is sync but called from within a tokio runtime.
    // block_in_place moves this work off the async thread pool, then
    // Handle::current().block_on() runs the async ZMQ call synchronously.
    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async move {
            let mut req = ReqSocket::new();
            req.connect(&endpoint).await?;

            let mut payload = serde_json::json!({
                "command": "stream_create",
                "name": name_owned,
            });
            if let Some(r) = retention_owned {
                payload["retention"] = serde_json::json!(r);
            }

            let bytes = serde_json::to_vec(&payload)?;
            req.send(ZmqMessage::from(bytes)).await?;

            tokio::time::timeout(std::time::Duration::from_secs(2), req.recv()).await??;

            Ok::<(), Box<dyn std::error::Error>>(())
        })
    });

    if let Err(e) = result {
        tracing::warn!(stream = %name, error = %e, "stream registration with broker failed");
    } else {
        tracing::info!(stream = %name, "stream registered with broker");
    }
}

/// Sanitize a string for use in a container name.
///
/// Replaces non-alphanumeric chars (except `-`) with `-`,
/// collapses multiple `-`, and trims leading/trailing `-`.
pub fn sanitize_name(name: &str) -> String {
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

/// Volume suffixes for named data volumes (ADR-027).
///
/// Each topology gets 5 named volumes: wal, users, skills, personas, context.
const VOLUME_SUFFIXES: &[&str] = &["wal", "users", "skills", "personas", "context"];

/// Build the Podman network name for a topology (ADR-024).
///
/// Format: `wh-<sanitized_topology_name>`
///
/// Each topology gets its own isolated Podman network. All containers
/// in the topology attach to this network for DNS-based discovery.
pub fn network_name(topology_name: &str) -> String {
    let topo = sanitize_name(topology_name);
    format!("wh-{topo}")
}

/// Create the topology Podman network idempotently (ADR-024).
///
/// Runs `podman network create <name> --ignore`. The `--ignore` flag
/// makes this safe to call repeatedly — if the network already exists,
/// the command succeeds silently.
///
/// Returns an error if podman is not found or the command fails.
#[tracing::instrument(skip_all, fields(topology = %topology_name))]
pub fn ensure_network(topology_name: &str) -> Result<(), DeployError> {
    let podman = find_podman()?;
    let net_name = network_name(topology_name);

    tracing::info!(network = %net_name, "ensuring topology network exists");
    run_podman_checked(
        podman,
        &["network", "create", &net_name, "--ignore"],
        PODMAN_CMD_TIMEOUT,
    )?;
    tracing::info!(network = %net_name, "topology network ready");
    Ok(())
}

/// Remove the topology Podman network (ADR-024).
///
/// Runs `podman network rm <name>`. Called during `deploy destroy` after
/// all containers have been stopped. Failure is logged but not fatal —
/// the network may already be removed or may have lingering containers.
#[tracing::instrument(skip_all, fields(topology = %topology_name))]
pub fn remove_network(topology_name: &str) -> Result<(), DeployError> {
    let podman = find_podman()?;
    let net_name = network_name(topology_name);

    tracing::info!(network = %net_name, "removing topology network");
    let output = run_podman(podman, &["network", "rm", &net_name], PODMAN_CMD_TIMEOUT)?;
    if output.status.success() {
        tracing::info!(network = %net_name, "topology network removed");
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(network = %net_name, error = %stderr, "failed to remove topology network");
    }
    Ok(())
}

/// Build the named volume names for a topology (ADR-027).
///
/// Returns a `Vec` of 5 volume names in deterministic order:
/// `wh-<sanitized_topology>-{wal, users, skills, personas, context}`.
pub fn volume_names(topology_name: &str) -> Vec<String> {
    let topo = sanitize_name(topology_name);
    VOLUME_SUFFIXES
        .iter()
        .map(|suffix| format!("wh-{topo}-{suffix}"))
        .collect()
}

/// Create all named data volumes for the topology idempotently (ADR-027).
///
/// Runs `podman volume create <name> --ignore` for each of the 5 volumes.
/// The `--ignore` flag makes this safe to call repeatedly — if a volume
/// already exists, the command succeeds silently.
///
/// Fails fast on the first volume creation error (same pattern as
/// `ensure_network()`).
#[tracing::instrument(skip_all, fields(topology = %topology_name))]
pub fn ensure_volumes(topology_name: &str) -> Result<(), DeployError> {
    let podman = find_podman()?;
    let names = volume_names(topology_name);

    tracing::info!("ensuring topology data volumes exist");
    for name in &names {
        run_podman_checked(
            podman,
            &["volume", "create", name, "--ignore"],
            PODMAN_CMD_TIMEOUT,
        )?;
    }
    tracing::info!(count = names.len(), "topology data volumes ready");
    Ok(())
}

/// Remove all named data volumes for the topology (ADR-027).
///
/// Runs `podman volume rm <name>` for each volume. Called during
/// `deploy destroy` after all containers and the network have been removed.
/// Best-effort: each volume removal is attempted independently. Failures
/// are logged as warnings but do not abort the remaining removals.
#[tracing::instrument(skip_all, fields(topology = %topology_name))]
pub fn remove_volumes(topology_name: &str) -> Result<(), DeployError> {
    let podman = find_podman()?;
    let names = volume_names(topology_name);

    tracing::info!("removing topology data volumes");
    for name in &names {
        let output = run_podman(podman, &["volume", "rm", name], PODMAN_CMD_TIMEOUT)?;
        if output.status.success() {
            tracing::info!(volume = %name, "volume removed");
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(volume = %name, error = %stderr, "failed to remove volume");
        }
    }
    Ok(())
}

/// Build the deterministic container name for an agent.
///
/// Format: `wh-<topology>-<agent>`
pub fn container_name(topology_name: &str, agent_name: &str) -> String {
    let topo = sanitize_name(topology_name);
    let agent = sanitize_name(agent_name);
    format!("wh-{topo}-{agent}")
}

/// Build the container image name for a surface kind (ADR-026).
///
/// `kind: telegram` → `ghcr.io/wheelhouse-paris/wh-telegram:latest`
pub fn surface_image(kind: &str) -> String {
    format!("{SURFACE_IMAGE_PREFIX}{kind}:{SURFACE_IMAGE_TAG}")
}

/// Build command arguments for `podman run` for a surface container (ADR-026).
///
/// Returns the argument list for starting a surface as a Podman container
/// on the topology network. Surface containers get:
/// - `--network wh-<topology>` for DNS-based discovery
/// - `-v wh-<topology>-users:/data/users` for user profile access
/// - `-e WH_URL=tcp://wh-broker:5555` (DNS broker address)
/// - `-e WH_SURFACE_NAME=<name>`, `-e WH_STREAM=<stream>`
/// - Any surface-specific env vars (e.g., `TELEGRAM_BOT_TOKEN`)
/// - Any extra_env vars (e.g., `CLAUDE_CODE_OAUTH_TOKEN`)
pub fn build_surface_run_args(
    topology_name: &str,
    surface: &crate::deploy::Surface,
    extra_env: &[(String, String)],
) -> Vec<String> {
    let name = container_name(topology_name, &surface.name);
    let net_name = network_name(topology_name);
    let users_volume = format!("wh-{}-users:/data/users", sanitize_name(topology_name));
    let image = surface_image(&surface.kind);

    let mut args = vec![
        "run".to_string(),
        "-d".to_string(),
        "--name".to_string(),
        name,
        "--network".to_string(),
        net_name,
        "-v".to_string(),
        users_volume,
        "-e".to_string(),
        format!("WH_URL={BROKER_DNS_URL}"),
        "-e".to_string(),
        format!("WH_SURFACE_NAME={}", surface.name),
    ];

    if !surface.stream.is_empty() {
        args.push("-e".to_string());
        args.push(format!("WH_STREAM={}", surface.stream));
    }

    // Inject caller-provided secrets/env vars (e.g. CLAUDE_CODE_OAUTH_TOKEN)
    for (key, value) in extra_env {
        args.push("-e".to_string());
        args.push(format!("{key}={value}"));
    }

    // Inject surface-specific env vars from topology spec
    if let Some(env_map) = &surface.env {
        for (key, value) in env_map {
            args.push("-e".to_string());
            args.push(format!("{key}={value}"));
        }
    }

    // Serialize multi-chat configuration as JSON env var (Telegram surfaces)
    if let Some(chats) = &surface.chats {
        if let Ok(chats_json) = serde_json::to_string(chats) {
            args.push("-e".to_string());
            args.push(format!("WH_CHATS={chats_json}"));
        }
    }

    args.push(image);
    args
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
/// When `network` is provided, adds `--network <name>` to attach the
/// container to the topology's isolated Podman network (ADR-024).
/// When `persona_path` is provided, adds a read-only volume mount and
/// `WH_PERSONA_PATH` environment variable for persona files.
/// `extra_env` is a list of additional `(KEY, VALUE)` pairs injected as `-e` flags
/// (used to pass secrets like `CLAUDE_CODE_OAUTH_TOKEN` from the CLI keychain).
#[allow(clippy::too_many_arguments)]
pub fn build_run_args(
    topology_name: &str,
    agent_name: &str,
    image: &str,
    streams: &[String],
    broker_url: Option<&str>,
    persona_path: Option<&str>,
    extra_env: &[(String, String)],
    network: Option<&str>,
) -> Vec<String> {
    let name = container_name(topology_name, agent_name);
    let url = broker_url.unwrap_or(BROKER_DNS_URL);
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

    // Inject caller-provided secrets/env vars (e.g. CLAUDE_CODE_OAUTH_TOKEN)
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

    // Attach to topology network (ADR-024)
    if let Some(net) = network {
        args.push("--network".to_string());
        args.push(net.to_string());
    }

    args.push(image.to_string());
    args
}

/// Start an agent container via Podman.
///
/// Uses `podman run -d` with the appropriate environment variables.
/// When `persona_path` is provided, mounts persona files read-only.
/// When `network` is provided, attaches the container to the named Podman network (ADR-024).
/// `extra_env` is forwarded to `build_run_args` for secret injection.
/// Timeout: 120s (image pull may be slow on first run).
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip_all, fields(agent = agent_name, topology = topology_name))]
pub fn podman_run(
    topology_name: &str,
    agent_name: &str,
    image: &str,
    streams: &[String],
    broker_url: Option<&str>,
    persona_path: Option<&str>,
    extra_env: &[(String, String)],
    network: Option<&str>,
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
        network,
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

/// Parse a component string like "surface telegram" to extract the surface name.
fn parse_surface_name(component: &str) -> Option<&str> {
    component.strip_prefix("surface ")
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
/// Used to pass secrets (e.g. `CLAUDE_CODE_OAUTH_TOKEN`) read from the CLI keychain.
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
    streams: &[crate::deploy::Stream],
    surfaces: &[crate::deploy::Surface],
    workspace_root: Option<&std::path::Path>,
    extra_env: &[(String, String)],
) -> ApplyResult {
    // Count stream additions upfront — streams require no container operation.
    let streams_created = changes
        .iter()
        .filter(|c| {
            parse_agent_name(&c.component).is_none()
                && parse_surface_name(&c.component).is_none()
                && c.op == "+"
        })
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
            surfaces_created: 0,
            surfaces_changed: 0,
            surfaces_destroyed: 0,
        };
    }

    // Create topology network (ADR-024) before any container starts.
    if let Err(e) = ensure_network(topology_name) {
        tracing::error!(error = %e, "failed to create topology network");
        eprintln!("Error: {e}");
        return ApplyResult {
            created: 0,
            changed: 0,
            destroyed: 0,
            streams_created,
            surfaces_created: 0,
            surfaces_changed: 0,
            surfaces_destroyed: 0,
        };
    }

    // Create named data volumes (ADR-027) before any container starts.
    if let Err(e) = ensure_volumes(topology_name) {
        tracing::error!(error = %e, "failed to create topology data volumes");
        eprintln!("Error: {e}");
        return ApplyResult {
            created: 0,
            changed: 0,
            destroyed: 0,
            streams_created,
            surfaces_created: 0,
            surfaces_changed: 0,
            surfaces_destroyed: 0,
        };
    }

    // Start the broker container before any agent containers (ADR-025).
    // Agents connect to the broker via DNS at startup, so it must be available first.
    if let Err(e) = ensure_broker_container(topology_name) {
        tracing::error!(error = %e, "broker is not available");
        eprintln!("Error: {e}");
        return ApplyResult {
            created: 0,
            changed: 0,
            destroyed: 0,
            streams_created,
            surfaces_created: 0,
            surfaces_changed: 0,
            surfaces_destroyed: 0,
        };
    }

    // Pre-compute topology network name (ADR-024) for all agent containers.
    let topo_network = network_name(topology_name);

    let mut result = ApplyResult {
        created: 0,
        changed: 0,
        destroyed: 0,
        streams_created,
        surfaces_created: 0,
        surfaces_changed: 0,
        surfaces_destroyed: 0,
    };

    for change in changes {
        // Surface changes — container lifecycle (ADR-026).
        if let Some(surface_name) = parse_surface_name(&change.component) {
            if change.op == "-" {
                let cname = container_name(topology_name, surface_name);
                match podman_stop(&cname) {
                    Ok(()) => result.surfaces_destroyed += 1,
                    Err(e) => tracing::error!(
                        surface = %surface_name,
                        error = %e,
                        "failed to stop surface container — retry with `wh deploy apply`"
                    ),
                }
                continue;
            }

            let Some(surface) = surfaces.iter().find(|s| s.name == surface_name) else {
                tracing::warn!(surface = %surface_name, "surface not found in topology — skipping");
                continue;
            };

            // CLI surface remains native — skip container creation (ADR-026 exception).
            if surface.kind == "cli" {
                tracing::debug!(surface = %surface.name, "CLI surface is native — skipping container creation");
                continue;
            }

            let run_args = build_surface_run_args(topology_name, surface, extra_env);
            let run_args_ref: Vec<&str> = run_args.iter().map(|s| s.as_str()).collect();

            match change.op.as_str() {
                "+" => {
                    match run_podman_checked(
                        find_podman().unwrap_or("podman"),
                        &run_args_ref,
                        PODMAN_RUN_TIMEOUT,
                    ) {
                        Ok(_) => {
                            result.surfaces_created += 1;
                            tracing::info!(surface = %surface.name, "surface container started");
                        }
                        Err(e) => tracing::error!(
                            surface = %surface.name,
                            error = %e,
                            "failed to start surface container — retry with `wh deploy apply`"
                        ),
                    }
                }
                "~" => {
                    let cname = container_name(topology_name, &surface.name);
                    let _ = podman_stop(&cname);
                    match run_podman_checked(
                        find_podman().unwrap_or("podman"),
                        &run_args_ref,
                        PODMAN_RUN_TIMEOUT,
                    ) {
                        Ok(_) => {
                            result.surfaces_changed += 1;
                            tracing::info!(surface = %surface.name, "surface container restarted");
                        }
                        Err(e) => tracing::error!(
                            surface = %surface.name,
                            error = %e,
                            "failed to restart surface container — retry with `wh deploy apply`"
                        ),
                    }
                }
                _ => {}
            }
            continue;
        }

        // Stream changes — register with broker (no container operation).
        if parse_agent_name(&change.component).is_none() {
            if change.op == "+" {
                // Component format: "stream <name>"
                let stream_name = change.component.trim_start_matches("stream ").to_string();
                let retention = streams
                    .iter()
                    .find(|s| s.name == stream_name)
                    .and_then(|s| s.retention.as_deref());
                register_stream_with_broker(&stream_name, retention);
            }
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
                    Some(BROKER_DNS_URL),
                    persona_abs.as_deref(),
                    extra_env,
                    Some(&topo_network),
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
                    Some(BROKER_DNS_URL),
                    persona_abs.as_deref(),
                    extra_env,
                    Some(&topo_network),
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
    fn network_name_formats_correctly() {
        assert_eq!(network_name("dev"), "wh-dev");
        assert_eq!(network_name("my-app"), "wh-my-app");
    }

    #[test]
    fn network_name_sanitizes_special_chars() {
        assert_eq!(network_name("my app"), "wh-my-app");
        assert_eq!(network_name("--bad--"), "wh-bad");
    }

    #[test]
    fn volume_names_formats_correctly() {
        let names = volume_names("dev");
        assert_eq!(
            names,
            vec![
                "wh-dev-wal",
                "wh-dev-users",
                "wh-dev-skills",
                "wh-dev-personas",
                "wh-dev-context",
            ]
        );
    }

    #[test]
    fn volume_names_sanitizes_special_chars() {
        let names = volume_names("my app");
        assert_eq!(
            names,
            vec![
                "wh-my-app-wal",
                "wh-my-app-users",
                "wh-my-app-skills",
                "wh-my-app-personas",
                "wh-my-app-context",
            ]
        );
    }

    #[test]
    fn volume_names_count() {
        let names = volume_names("dev");
        assert_eq!(names.len(), 5);
    }

    #[test]
    fn volume_suffixes_constant_has_five_entries() {
        assert_eq!(VOLUME_SUFFIXES.len(), 5);
        assert_eq!(VOLUME_SUFFIXES[0], "wal");
        assert_eq!(VOLUME_SUFFIXES[1], "users");
        assert_eq!(VOLUME_SUFFIXES[2], "skills");
        assert_eq!(VOLUME_SUFFIXES[3], "personas");
        assert_eq!(VOLUME_SUFFIXES[4], "context");
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
            None,
        );
        assert_eq!(args[0], "run");
        assert_eq!(args[1], "-d");
        assert_eq!(args[2], "--name");
        assert_eq!(args[3], "wh-dev-researcher");
        assert_eq!(args[4], "-e");
        // ADR-025: default broker URL uses DNS within the topology network.
        assert_eq!(args[5], "WH_URL=tcp://wh-broker:5555");
        assert_eq!(args[6], "-e");
        assert_eq!(args[7], "WH_AGENT_NAME=researcher");
        assert_eq!(args[8], "-e");
        assert_eq!(args[9], "WH_STREAMS=main");
        assert_eq!(args[10], "researcher:latest");
        // No --network when network is None
        assert!(
            !args.iter().any(|a| a == "--network"),
            "should not have --network without network param"
        );
    }

    #[test]
    fn build_run_args_with_network() {
        let args = build_run_args(
            "dev",
            "researcher",
            "researcher:latest",
            &["main".to_string()],
            None,
            None,
            &[],
            Some("wh-dev"),
        );
        let net_idx = args
            .iter()
            .position(|a| a == "--network")
            .expect("should have --network flag");
        assert_eq!(args[net_idx + 1], "wh-dev");
        // Image should be the last arg
        assert_eq!(args.last().unwrap(), "researcher:latest");
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
            None,
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
            None,
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
            None,
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

    #[tokio::test(flavor = "multi_thread")]
    async fn provision_containers_skips_stream_changes() {
        let changes = vec![Change {
            op: "+".to_string(),
            component: "stream main".to_string(),
            field: None,
            from: None,
            to: None,
        }];
        let result = provision_containers("dev", &changes, &[], &[], &[], None, &[]);
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
            surfaces_created: 0,
            surfaces_changed: 0,
            surfaces_destroyed: 0,
        };
        assert_eq!(
            result.to_string(),
            "1 created \u{00B7} 0 changed \u{00B7} 2 destroyed \u{00B7} 1 streams \u{00B7} 0 surfaces created \u{00B7} 0 surfaces changed \u{00B7} 0 surfaces destroyed"
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
        let result = provision_containers("dev", &changes, &[], &[], &[], None, &[]);
        // Should skip gracefully, not panic, and not increment created
        assert_eq!(result.created, 0);
    }

    #[test]
    fn surface_image_formats_correctly() {
        // ADR-026: surface image derived from kind.
        assert_eq!(
            surface_image("telegram"),
            "ghcr.io/wheelhouse-paris/wh-telegram:latest"
        );
        assert_eq!(
            surface_image("discord"),
            "ghcr.io/wheelhouse-paris/wh-discord:latest"
        );
    }

    #[test]
    fn build_surface_run_args_correct() {
        let surface = crate::deploy::Surface {
            name: "telegram".to_string(),
            kind: "telegram".to_string(),
            stream: "main".to_string(),
            env: Some(std::collections::BTreeMap::from([(
                "TELEGRAM_BOT_TOKEN".to_string(),
                "tok123".to_string(),
            )])),
            chats: None,
        };
        let args = build_surface_run_args("dev", &surface, &[]);

        assert_eq!(args[0], "run");
        assert_eq!(args[1], "-d");
        assert_eq!(args[2], "--name");
        assert_eq!(args[3], "wh-dev-telegram");
        assert_eq!(args[4], "--network");
        assert_eq!(args[5], "wh-dev");
        assert_eq!(args[6], "-v");
        assert_eq!(args[7], "wh-dev-users:/data/users");
        assert_eq!(args[8], "-e");
        assert_eq!(args[9], "WH_URL=tcp://wh-broker:5555");
        assert_eq!(args[10], "-e");
        assert_eq!(args[11], "WH_SURFACE_NAME=telegram");
        assert_eq!(args[12], "-e");
        assert_eq!(args[13], "WH_STREAM=main");
        // Surface-specific env
        assert!(
            args.iter().any(|a| a == "TELEGRAM_BOT_TOKEN=tok123"),
            "should include surface spec env"
        );
        // Image is last
        assert_eq!(
            args.last().unwrap(),
            "ghcr.io/wheelhouse-paris/wh-telegram:latest"
        );
    }

    #[test]
    fn build_surface_run_args_with_extra_env() {
        let surface = crate::deploy::Surface {
            name: "telegram".to_string(),
            kind: "telegram".to_string(),
            stream: "main".to_string(),
            env: None,
            chats: None,
        };
        let extra = vec![("SECRET_KEY".to_string(), "secret-val".to_string())];
        let args = build_surface_run_args("dev", &surface, &extra);

        assert!(
            args.iter().any(|a| a == "SECRET_KEY=secret-val"),
            "should include extra_env"
        );
    }

    #[test]
    fn build_surface_run_args_with_chats() {
        let surface = crate::deploy::Surface {
            name: "telegram".to_string(),
            kind: "telegram".to_string(),
            stream: String::new(),
            env: None,
            chats: Some(vec![crate::deploy::SurfaceChatConfig {
                id: "@user".to_string(),
                stream: Some("dm-stream".to_string()),
                threads: None,
            }]),
        };
        let args = build_surface_run_args("dev", &surface, &[]);

        // Should have WH_CHATS env var with JSON
        let chats_arg = args.iter().find(|a| a.starts_with("WH_CHATS="));
        assert!(
            chats_arg.is_some(),
            "should have WH_CHATS env var for multi-chat config"
        );
        let chats_json = chats_arg.unwrap().strip_prefix("WH_CHATS=").unwrap();
        assert!(
            chats_json.contains("@user"),
            "WH_CHATS should contain chat id"
        );
        // No WH_STREAM when stream is empty
        assert!(
            !args.iter().any(|a| a == "WH_STREAM="),
            "should not have empty WH_STREAM"
        );
    }

    #[test]
    fn build_surface_run_args_no_stream_when_empty() {
        let surface = crate::deploy::Surface {
            name: "telegram".to_string(),
            kind: "telegram".to_string(),
            stream: String::new(),
            env: None,
            chats: None,
        };
        let args = build_surface_run_args("dev", &surface, &[]);

        assert!(
            !args.iter().any(|a| a.starts_with("WH_STREAM=")),
            "should not inject WH_STREAM when stream is empty"
        );
    }

    #[test]
    fn parse_surface_name_works() {
        assert_eq!(parse_surface_name("surface telegram"), Some("telegram"));
        assert_eq!(parse_surface_name("surface cli"), Some("cli"));
        assert_eq!(parse_surface_name("agent researcher"), None);
        assert_eq!(parse_surface_name("stream main"), None);
    }

    #[test]
    fn broker_dns_url_uses_container_name() {
        // ADR-025: agents connect to broker via DNS within the topology network.
        assert_eq!(BROKER_DNS_URL, "tcp://wh-broker:5555");
    }

    #[test]
    fn broker_container_name_constant() {
        // ADR-024: broker container has a fixed name, no topology prefix.
        assert_eq!(BROKER_CONTAINER_NAME, "wh-broker");
    }

    #[test]
    fn broker_image_constant() {
        // ADR-028: broker image from GHCR.
        assert!(BROKER_IMAGE.starts_with("ghcr.io/wheelhouse-paris/wh-broker:"));
    }

    #[test]
    fn host_broker_url_uses_localhost() {
        // Host-side processes (surfaces, CLI) connect via published ports.
        assert_eq!(HOST_BROKER_URL, "tcp://127.0.0.1:5555");
    }

    #[test]
    fn apply_result_display_with_surfaces() {
        let result = ApplyResult {
            created: 1,
            changed: 0,
            destroyed: 0,
            streams_created: 1,
            surfaces_created: 2,
            surfaces_changed: 1,
            surfaces_destroyed: 0,
        };
        let display = result.to_string();
        assert!(
            display.contains("2 surfaces created"),
            "should show surfaces_created separately: {display}"
        );
        assert!(
            display.contains("1 surfaces changed"),
            "should show surfaces_changed separately: {display}"
        );
        assert!(
            display.contains("0 surfaces destroyed"),
            "should show surfaces_destroyed separately: {display}"
        );
    }

    #[test]
    fn surface_env_merge_includes_spec_entries_via_build_args() {
        // ADR-026: verify build_surface_run_args includes all env vars.
        let extra_env: Vec<(String, String)> = vec![(
            "CLAUDE_CODE_OAUTH_TOKEN".to_string(),
            "oauth-token-xxx".to_string(),
        )];
        let surface = crate::deploy::Surface {
            name: "telegram".to_string(),
            kind: "telegram".to_string(),
            stream: "main".to_string(),
            env: Some(std::collections::BTreeMap::from([
                ("TELEGRAM_BOT_TOKEN".to_string(), "tok123".to_string()),
                ("CHAT_ID".to_string(), "456".to_string()),
            ])),
            chats: None,
        };

        let args = build_surface_run_args("dev", &surface, &extra_env);

        // Verify all expected env vars are present in the args
        assert!(
            args.iter()
                .any(|a| a == "CLAUDE_CODE_OAUTH_TOKEN=oauth-token-xxx"),
            "should carry over extra_env"
        );
        assert!(
            args.iter().any(|a| a == "WH_SURFACE_NAME=telegram"),
            "should inject WH_SURFACE_NAME"
        );
        assert!(
            args.iter().any(|a| a == "WH_STREAM=main"),
            "should inject WH_STREAM"
        );
        assert!(
            args.iter().any(|a| a == "TELEGRAM_BOT_TOKEN=tok123"),
            "should include surface spec env"
        );
        assert!(
            args.iter().any(|a| a == "CHAT_ID=456"),
            "should include all surface spec env entries"
        );
    }

    #[test]
    fn stream_count_excludes_surface_and_agent_components() {
        // Verify stream counting logic: only components that are NOT agents
        // and NOT surfaces with op "+" count as stream additions.
        let changes = vec![
            Change {
                op: "+".to_string(),
                component: "stream main".to_string(),
                field: None,
                from: None,
                to: None,
            },
            Change {
                op: "+".to_string(),
                component: "surface telegram".to_string(),
                field: None,
                from: None,
                to: None,
            },
            Change {
                op: "+".to_string(),
                component: "agent researcher".to_string(),
                field: None,
                from: None,
                to: None,
            },
        ];
        // Count streams using the same logic as provision_containers
        let streams_created = changes
            .iter()
            .filter(|c| {
                parse_agent_name(&c.component).is_none()
                    && parse_surface_name(&c.component).is_none()
                    && c.op == "+"
            })
            .count();
        assert_eq!(streams_created, 1, "only 'stream main' should count");
    }
}
