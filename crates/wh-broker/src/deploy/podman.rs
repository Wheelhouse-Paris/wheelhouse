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

/// Default port mappings for the broker container (ADR-025).
///
/// Used when the topology's `broker.ports` is empty or absent.
const DEFAULT_BROKER_PORTS: &[&str] = &[
    "127.0.0.1:5555:5555",
    "127.0.0.1:5556:5556",
    "127.0.0.1:5557:5557",
];

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
pub fn ensure_broker_container(
    topology_name: &str,
    broker: Option<&crate::deploy::BrokerSpec>,
) -> Result<(), DeployError> {
    // Idempotent: skip if already running.
    if podman_is_running(BROKER_CONTAINER_NAME)? {
        tracing::info!("broker container already running");
        return Ok(());
    }

    let podman = find_podman()?;
    let net_name = network_name(topology_name);
    let wal_volume = format!("wh-{}-wal", sanitize_name(topology_name));
    let volume_mount = format!("{wal_volume}:/data");

    // Use topology-specified image or fall back to default (ADR-029).
    let image = broker.map(|b| b.image.as_str()).unwrap_or(BROKER_IMAGE);

    // Build args with topology-specified or default ports (ADR-029).
    let custom_ports = broker.map(|b| &b.ports).filter(|p| !p.is_empty());

    let mut args: Vec<String> = vec![
        "run".to_string(),
        "-d".to_string(),
        "--name".to_string(),
        BROKER_CONTAINER_NAME.to_string(),
        "--network".to_string(),
        net_name,
    ];

    if let Some(ports) = custom_ports {
        for port in ports {
            args.push("-p".to_string());
            args.push(port.clone());
        }
    } else {
        for port in DEFAULT_BROKER_PORTS {
            args.push("-p".to_string());
            args.push((*port).to_string());
        }
    }

    args.push("-v".to_string());
    args.push(volume_mount);
    args.push("-e".to_string());
    args.push("WH_DATA_DIR=/data".to_string());
    args.push(image.to_string());

    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    tracing::info!("starting broker container");
    eprintln!("  Starting Wheelhouse broker container...");
    run_podman_checked(podman, &arg_refs, PODMAN_RUN_TIMEOUT)?;

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
/// Each topology gets 6 named volumes: wal, users, skills, personas, context, platform.
const VOLUME_SUFFIXES: &[&str] = &["wal", "users", "skills", "personas", "context", "platform"];

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

/// Build the platform volume name for a topology.
fn platform_volume_name(topology_name: &str) -> String {
    let topo = sanitize_name(topology_name);
    format!("wh-{topo}-platform")
}

/// Populate the platform data volume with capabilities.json and cli-reference.md.
///
/// Uses a temporary Alpine container to copy files into the named volume.
/// The `wh` binary on the host generates the content; a throwaway container
/// writes it into the volume so agent containers can mount it at `/etc/wh:ro`.
///
/// This is idempotent — the volume contents are overwritten on each deploy
/// to stay in sync with the installed `wh` version.
#[tracing::instrument(skip_all, fields(topology = %topology_name))]
pub fn populate_platform_volume(topology_name: &str) -> Result<(), DeployError> {
    let podman = find_podman()?;
    let vol = platform_volume_name(topology_name);
    let helper = format!("wh-platform-init-{}", sanitize_name(topology_name));

    // Use the current binary's path to invoke subcommands (avoids PATH issues)
    let wh_bin = std::env::current_exe().map_err(|e| {
        DeployError::ApplyFailed(format!("cannot resolve wh binary path: {e}"))
    })?;

    // Generate capabilities.json from the host wh binary
    let capabilities_output = std::process::Command::new(&wh_bin)
        .args(["capabilities", "--format", "json"])
        .output();
    let capabilities_json = match capabilities_output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
        _ => {
            tracing::warn!("wh capabilities failed — platform volume will lack L0 layer");
            String::new()
        }
    };

    // Generate cli-reference.md from the host wh binary
    let reference_output = std::process::Command::new(&wh_bin)
        .args(["reference"])
        .output();
    let cli_reference = match reference_output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
        _ => {
            tracing::warn!("wh reference failed — platform volume will lack L1 layer");
            String::new()
        }
    };

    if capabilities_json.is_empty() && cli_reference.is_empty() {
        tracing::warn!("no platform context generated — skipping volume population");
        return Ok(());
    }

    // Build a shell script that writes both files into /etc/wh inside the volume
    let mut script = String::from("mkdir -p /etc/wh && ");
    if !capabilities_json.is_empty() {
        // Escape single quotes in JSON for shell safety
        let escaped = capabilities_json.replace('\'', "'\\''");
        script.push_str(&format!("printf '%s' '{}' > /etc/wh/capabilities.json && ", escaped));
    }
    if !cli_reference.is_empty() {
        let escaped = cli_reference.replace('\'', "'\\''");
        script.push_str(&format!("printf '%s' '{}' > /etc/wh/cli-reference.md && ", escaped));
    }
    script.push_str("echo done");

    // Run a throwaway Alpine container that mounts the volume and writes the files
    let args = [
        "run", "--rm",
        "--name", &helper,
        "-v", &format!("{vol}:/etc/wh"),
        "alpine:latest",
        "sh", "-c", &script,
    ];

    tracing::info!("populating platform volume with capabilities + CLI reference");
    run_podman_checked(podman, &args, PODMAN_RUN_TIMEOUT)?;
    tracing::info!("platform volume populated");
    Ok(())
}

/// Build the personas volume name for a topology.
fn personas_volume_name(topology_name: &str) -> String {
    let topo = sanitize_name(topology_name);
    format!("wh-{topo}-personas")
}

/// Populate the personas data volume with persona files from the workspace.
///
/// For each agent with a `persona` path configured, copies SOUL.md, IDENTITY.md,
/// and MEMORY.md from the workspace into a per-agent subdirectory inside the
/// `wh-<topology>-personas` named volume.
///
/// If the workspace has a git remote, initializes a sparse-checkout git repo
/// inside each agent's subdirectory so the agent can commit+push MEMORY.md changes.
///
/// Layout: `/personas/<agent_name>/{SOUL.md, IDENTITY.md, MEMORY.md}`
///
/// This is idempotent — files are overwritten on each deploy (except MEMORY.md
/// which is preserved if it already exists in the volume).
#[tracing::instrument(skip_all, fields(topology = %topology_name))]
pub fn populate_personas_volume(
    topology_name: &str,
    agents: &[crate::deploy::Agent],
    workspace_root: Option<&std::path::Path>,
) -> Result<(), DeployError> {
    let ws_root = match workspace_root {
        Some(root) => root,
        None => {
            tracing::debug!("no workspace root — skipping personas volume population");
            return Ok(());
        }
    };

    // Collect agents that have persona configured
    let agents_with_persona: Vec<_> = agents
        .iter()
        .filter_map(|a| a.persona.as_ref().map(|p| (a.name.as_str(), p.as_str())))
        .collect();

    if agents_with_persona.is_empty() {
        tracing::debug!("no agents with persona configured — skipping");
        return Ok(());
    }

    let podman = find_podman()?;
    let vol = personas_volume_name(topology_name);
    let helper = format!("wh-personas-init-{}", sanitize_name(topology_name));

    // Build a shell script that creates per-agent directories and writes persona files
    let mut script = String::new();

    for (agent_name, persona_rel) in &agents_with_persona {
        let persona_dir = ws_root.join(persona_rel);

        // Create agent subdirectory
        script.push_str(&format!("mkdir -p /personas/{agent_name} && "));

        // Copy each persona file (SOUL.md, IDENTITY.md are overwritten; MEMORY.md preserved)
        for filename in &["SOUL.md", "IDENTITY.md"] {
            let file_path = persona_dir.join(filename);
            if file_path.exists() {
                let content = std::fs::read_to_string(&file_path).map_err(|e| {
                    DeployError::ApplyFailed(format!(
                        "cannot read {filename} for agent {agent_name}: {e}"
                    ))
                })?;
                let escaped = content.replace('\'', "'\\''");
                script.push_str(&format!(
                    "printf '%s' '{escaped}' > /personas/{agent_name}/{filename} && "
                ));
            }
        }

        // MEMORY.md: only write if not already present in volume (preserve agent changes)
        let memory_path = persona_dir.join("MEMORY.md");
        if memory_path.exists() {
            let content = std::fs::read_to_string(&memory_path).map_err(|e| {
                DeployError::ApplyFailed(format!(
                    "cannot read MEMORY.md for agent {agent_name}: {e}"
                ))
            })?;
            let escaped = content.replace('\'', "'\\''");
            script.push_str(&format!(
                "[ -f /personas/{agent_name}/MEMORY.md ] || printf '%s' '{escaped}' > /personas/{agent_name}/MEMORY.md && "
            ));
        } else {
            // Initialize empty MEMORY.md if not present (FR61)
            script.push_str(&format!(
                "[ -f /personas/{agent_name}/MEMORY.md ] || touch /personas/{agent_name}/MEMORY.md && "
            ));
        }
    }

    script.push_str("echo done");

    // Run a throwaway Alpine container to populate the volume
    let args = [
        "run", "--rm",
        "--name", &helper,
        "-v", &format!("{vol}:/personas"),
        "alpine:latest",
        "sh", "-c", &script,
    ];

    tracing::info!(
        agents = agents_with_persona.len(),
        "populating personas volume"
    );
    run_podman_checked(podman, &args, PODMAN_RUN_TIMEOUT)?;
    tracing::info!("personas volume populated");
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

    // Inject caller-provided secrets/env vars (e.g. CLAUDE_CODE_OAUTH_TOKEN).
    // For file-path env vars (e.g. WH_TELEGRAM_ROUTING_FILE), bind-mount the
    // host file into the container and rewrite the env var to the container path.
    for (key, value) in extra_env {
        if key == "WH_TELEGRAM_ROUTING_FILE" {
            let container_path = "/etc/wh/telegram-routing.json";
            args.push("-v".to_string());
            args.push(format!("{value}:{container_path}:ro"));
            args.push("-e".to_string());
            args.push(format!("{key}={container_path}"));
        } else {
            args.push("-e".to_string());
            args.push(format!("{key}={value}"));
        }
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
/// When `has_persona` is true, mounts the personas named volume and sets
/// `WH_PERSONA_PATH` to the agent's subdirectory inside the volume.
/// When `has_context` is true, mounts the context named volume read-only
/// and sets `WH_CONTEXT_PATH` for per-stream context files.
/// `extra_env` is a list of additional `(KEY, VALUE)` pairs injected as `-e` flags
/// (used to pass secrets like `CLAUDE_CODE_OAUTH_TOKEN` from the CLI keychain).
#[allow(clippy::too_many_arguments)]
pub fn build_run_args(
    topology_name: &str,
    agent_name: &str,
    image: &str,
    streams: &[String],
    broker_url: Option<&str>,
    has_persona: bool,
    has_context: bool,
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

    // Mount personas named volume with per-agent subdirectory (ADR-027)
    // Read-write: agent needs to write MEMORY.md and commit via git (FR62)
    if has_persona {
        let personas_vol = personas_volume_name(topology_name);
        args.push("-v".to_string());
        args.push(format!("{personas_vol}:/personas"));
        args.push("-e".to_string());
        args.push(format!("WH_PERSONA_PATH=/personas/{agent_name}"));
    }

    // Mount context named volume read-only (ADR-021, ADR-027)
    if has_context {
        let context_vol = context_volume_name(topology_name);
        args.push("-v".to_string());
        args.push(format!("{context_vol}:/context:ro"));
        args.push("-e".to_string());
        args.push("WH_CONTEXT_PATH=/context".to_string());
    }

    // Mount platform volume at /etc/wh (read-only) for L0/L1 context injection (ADR-033)
    let platform_vol = platform_volume_name(topology_name);
    args.push("-v".to_string());
    args.push(format!("{platform_vol}:/etc/wh:ro"));

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
/// When `has_persona` is true, mounts the personas named volume.
/// When `has_context` is true, mounts the context named volume read-only.
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
    has_persona: bool,
    has_context: bool,
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
        has_persona,
        has_context,
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


/// Build the context volume name for a topology.
fn context_volume_name(topology_name: &str) -> String {
    let topo = sanitize_name(topology_name);
    format!("wh-{topo}-context")
}

/// Populate the context data volume from the workspace `.wh/context/` directory.
///
/// Copies all per-stream `CONTEXT.md` files from `.wh/context/<stream>/CONTEXT.md`
/// into the `wh-<topology>-context` named volume, preserving the directory structure.
///
/// This is idempotent — files are overwritten on each deploy to stay in sync
/// with the workspace.
#[tracing::instrument(skip_all, fields(topology = %topology_name))]
pub fn populate_context_volume(
    topology_name: &str,
    workspace_root: Option<&std::path::Path>,
) -> Result<(), DeployError> {
    let ws_root = match workspace_root {
        Some(root) => root,
        None => {
            tracing::debug!("no workspace root — skipping context volume population");
            return Ok(());
        }
    };

    let context_dir = ws_root.join(".wh").join("context");
    if !context_dir.is_dir() {
        tracing::debug!("no .wh/context/ directory — skipping");
        return Ok(());
    }

    // Collect all stream context files
    let mut entries: Vec<(String, String)> = Vec::new();
    if let Ok(dir_iter) = std::fs::read_dir(&context_dir) {
        for entry in dir_iter.flatten() {
            if entry.path().is_dir() {
                let stream_name = entry.file_name().to_string_lossy().to_string();
                let context_file = entry.path().join("CONTEXT.md");
                if context_file.exists() {
                    let content = std::fs::read_to_string(&context_file).map_err(|e| {
                        DeployError::ApplyFailed(format!(
                            "cannot read CONTEXT.md for stream {stream_name}: {e}"
                        ))
                    })?;
                    entries.push((stream_name, content));
                }
            }
        }
    }

    if entries.is_empty() {
        tracing::debug!("no CONTEXT.md files found — skipping");
        return Ok(());
    }

    let podman = find_podman()?;
    let vol = context_volume_name(topology_name);
    let helper = format!("wh-context-init-{}", sanitize_name(topology_name));

    // Build a shell script that writes each stream's CONTEXT.md into the volume
    let mut script = String::new();
    for (stream_name, content) in &entries {
        let escaped = content.replace('\'', "'\\''");
        script.push_str(&format!(
            "mkdir -p /context/{stream_name} && printf '%s' '{escaped}' > /context/{stream_name}/CONTEXT.md && "
        ));
    }
    script.push_str("echo done");

    let args = [
        "run", "--rm",
        "--name", &helper,
        "-v", &format!("{vol}:/context"),
        "alpine:latest",
        "sh", "-c", &script,
    ];

    tracing::info!(streams = entries.len(), "populating context volume");
    run_podman_checked(podman, &args, PODMAN_RUN_TIMEOUT)?;
    tracing::info!("context volume populated");
    Ok(())
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
#[allow(clippy::too_many_arguments)]
pub fn provision_containers(
    topology_name: &str,
    changes: &[Change],
    agents: &[crate::deploy::Agent],
    streams: &[crate::deploy::Stream],
    surfaces: &[crate::deploy::Surface],
    workspace_root: Option<&std::path::Path>,
    extra_env: &[(String, String)],
    broker: Option<&crate::deploy::BrokerSpec>,
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

    // Populate platform volume with capabilities.json + cli-reference.md (ADR-033).
    // Best-effort: failure is logged but does not block deployment.
    if let Err(e) = populate_platform_volume(topology_name) {
        tracing::warn!(error = %e, "failed to populate platform volume — agents will lack L0/L1 context");
    }

    // Populate personas volume from workspace persona directories (ADR-027).
    // Best-effort: failure is logged but does not block deployment.
    if let Err(e) = populate_personas_volume(topology_name, agents, workspace_root) {
        tracing::warn!(error = %e, "failed to populate personas volume — agents will lack persona files");
    }

    // Populate context volume from workspace .wh/context/ directory (ADR-021, ADR-027).
    // Best-effort: failure is logged but does not block deployment.
    if let Err(e) = populate_context_volume(topology_name, workspace_root) {
        tracing::warn!(error = %e, "failed to populate context volume — agents will lack stream context");
    }

    // Start the broker container before any agent containers (ADR-025).
    // Agents connect to the broker via DNS at startup, so it must be available first.
    if let Err(e) = ensure_broker_container(topology_name, broker) {
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
                        "failed to stop surface container — retry with `wh topology apply`"
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
                            "failed to start surface container — retry with `wh topology apply`"
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
                            "failed to restart surface container — retry with `wh topology apply`"
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

                // Check if agent has persona configured (volume already populated)
                let has_persona = agent.persona.is_some();
                // Context volume is already populated at deploy time
                let has_context = true;

                match podman_run(
                    topology_name,
                    &agent.name,
                    &agent.image,
                    &agent.streams,
                    Some(BROKER_DNS_URL),
                    has_persona,
                    has_context,
                    extra_env,
                    Some(&topo_network),
                ) {
                    Ok(()) => result.created += 1,
                    Err(e) => {
                        tracing::error!(
                            agent = %agent.name,
                            error = %e,
                            "failed to start agent container — retry with `wh topology apply`"
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
                            "failed to stop agent container — retry with `wh topology apply`"
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
                // Check if agent has persona configured (volume already populated)
                let has_persona = agent.persona.is_some();
                // Context volume is already populated at deploy time
                let has_context = true;

                // Stop old
                let _ = podman_stop(&name);
                // Start new
                match podman_run(
                    topology_name,
                    &agent.name,
                    &agent.image,
                    &agent.streams,
                    Some(BROKER_DNS_URL),
                    has_persona,
                    has_context,
                    extra_env,
                    Some(&topo_network),
                ) {
                    Ok(()) => result.changed += 1,
                    Err(e) => {
                        tracing::error!(
                            agent = %agent.name,
                            error = %e,
                            "failed to restart agent container — retry with `wh topology apply`"
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
            false,
            false,
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
        // Image is always the last arg (after volume mounts)
        assert_eq!(args.last().unwrap(), "researcher:latest");
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
            false,
            false,
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
            false,
            false,
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
            true,
            false,
            &[],
            None,
        );
        // Should contain personas named volume mount
        let v_idx = args
            .iter()
            .position(|a| a == "-v")
            .expect("should have -v flag");
        assert!(
            args[v_idx + 1].contains("personas"),
            "volume mount should reference personas named volume"
        );
        // Should contain WH_PERSONA_PATH env pointing to agent subdirectory
        assert!(
            args.iter().any(|a| a == "WH_PERSONA_PATH=/personas/donna"),
            "should have WH_PERSONA_PATH env var with agent subdirectory"
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
            false,
            false,
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
            source_file: None,
        }];
        let result = provision_containers("dev", &changes, &[], &[], &[], None, &[], None);
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
            source_file: None,
        }];
        let result = provision_containers("dev", &changes, &[], &[], &[], None, &[], None);
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
    fn build_surface_run_args_mounts_routing_file() {
        let surface = crate::deploy::Surface {
            name: "telegram".to_string(),
            kind: "telegram".to_string(),
            stream: "main".to_string(),
            env: None,
            chats: None,
        };
        let extra = vec![(
            "WH_TELEGRAM_ROUTING_FILE".to_string(),
            "/host/path/telegram-routing.json".to_string(),
        )];
        let args = build_surface_run_args("dev", &surface, &extra);

        assert!(
            args.iter()
                .any(|a| a == "/host/path/telegram-routing.json:/etc/wh/telegram-routing.json:ro"),
            "should bind-mount routing file into container"
        );
        assert!(
            args.iter()
                .any(|a| a == "WH_TELEGRAM_ROUTING_FILE=/etc/wh/telegram-routing.json"),
            "should rewrite env var to container path"
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
                source_file: None,
            },
            Change {
                op: "+".to_string(),
                component: "surface telegram".to_string(),
                field: None,
                from: None,
                to: None,
                source_file: None,
            },
            Change {
                op: "+".to_string(),
                component: "agent researcher".to_string(),
                field: None,
                from: None,
                to: None,
                source_file: None,
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
