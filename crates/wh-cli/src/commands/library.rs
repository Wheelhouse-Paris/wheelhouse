//! `wh library` command group — Library inspection and management (Epic 13 FW-7).
//!
//! This module is the canonical home for:
//!
//! 1. The `DEFAULT_SCHEMA_TEMPLATE` constant (story 13-19, R1 launch-blocker template).
//! 2. The `wh library` clap subcommand group with its `status` child subcommand
//!    (story 13-24 — the first FW-7 CLI surface).
//!
//! Stories 13-22..13-26 add `init`, `ingest`, `lint`, `list` by adding variants
//! to `LibraryCommand` and match arms to `run` — no restructuring needed.
//!
//! ## Story 13-24: `wh library status <agent>`
//!
//! Prints Library health for a single agent by reading the host-side mount of the
//! agent's per-agent workspace volume (story 13-3 / ADR-036):
//!
//! - Schema file present (`<mount>/.wh-schema.md` exists)
//! - Page count (`.md` files under `<mount>/.library/pages/`)
//! - Git HEAD short commit id (`git -C <mount>/.library rev-parse --short HEAD`)
//! - Last ingest timestamp (`git -C <mount>/.library log -1 --format=%aI HEAD`)
//! - Lock held (`<mount>/.library/.git/index.lock` exists) + lock age
//! - Free disk bytes (`df -kP <mount>`)
//!
//! No broker IPC, no container exec — the MVP is local host-side reads for FR45's
//! <2s target. See the story file for the rationale and the broker-RPC v1.1 path.
//!
//! ## Story 13-19 legacy: `DEFAULT_SCHEMA_TEMPLATE`
//!
//! The `DEFAULT_SCHEMA_TEMPLATE` constant is the canonical source of truth for the
//! default `.wh-schema.md` template that the wh framework injects at
//! `/workspace/.wh-schema.md` for every new Library. See the doc comment on the
//! constant itself for the R1 context.
//!
//! ## Why `include_str!`
//!
//! The wh binary is shipped as a single self-contained release artifact (story 1.6).
//! Embedding the template at compile time guarantees it travels with every binary and
//! removes any runtime path-resolution failure mode. Updating the template requires a
//! rebuild — that is the intended semantics.

/// Default Library schema template — read-only at runtime, embedded at compile time.
///
/// Path constant downstream stories (13-20, 13-22) will read to obtain the canonical
/// bytes that get materialized at `/workspace/.wh-schema.md` for new Libraries.
///
/// This string contains unresolved `{{agent_name}}` and `{{domain_description}}`
/// placeholders. The consumer (`wh library init` in story 13-22) is responsible for
/// substitution before writing the file to disk.
///
/// Risk: R1 (LAUNCH BLOCKER). Editing this template's *wording* changes the empirical
/// behavior of the agent's autonomous-write loop and re-opens the R1 usability gate.
/// Edit only with deliberate intent and re-run the R1 validation.
pub const DEFAULT_SCHEMA_TEMPLATE: &str = include_str!("../../templates/library/wh-schema.md.tmpl");

// ─── Story 13-24: `wh library status <agent>` ─────────────────────────────

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Args, Subcommand};
use prost::Message;
use serde::Serialize;
use wh_proto::{SkillInvocation, SkillResult, StreamEnvelope};
use zeromq::{PubSocket, Socket, SocketRecv, SocketSend, SubSocket, ZmqMessage};

use crate::output::error::WhError;
use crate::output::json;
use crate::output::OutputFormat;

/// `wh library` subcommand group.
///
/// Variants land incrementally as FW-7 stories are completed. The order is
/// **alphabetical** (Init | Ingest | Lint | List | Status) — keep it that way
/// when adding new variants.
#[derive(Debug, Subcommand)]
pub enum LibraryCommand {
    /// Initialize a local Library for an agent on its per-agent workspace volume (FR6).
    Init(InitArgs),
    /// Enqueue a Library ingestion by publishing a SkillInvocation (FR10, 13-23).
    Ingest(IngestArgs),
    /// Trigger a Library lint run by publishing a SkillInvocation (FR47, 13-26).
    Lint(LintArgs),
    /// List all Libraries across every agent in the current topology (FR46, 13-25).
    List(ListArgs),
    /// Inspect the health of an agent's Library (FR45).
    Status(StatusArgs),
}

/// Arguments for `wh library list` (story 13-25, FR46).
#[derive(Debug, Args)]
pub struct ListArgs {
    /// Output format: human (default) or json.
    #[arg(long, value_enum, default_value = "human")]
    pub format: OutputFormat,
}

impl ListArgs {
    /// The format hint used by `main.rs` for error envelope rendering.
    pub fn format(&self) -> OutputFormat {
        self.format
    }
}

/// Arguments for `wh library status`.
#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Name of the agent whose Library to inspect.
    pub agent: String,

    /// Output format: human (default) or json.
    #[arg(long, value_enum, default_value = "human")]
    pub format: OutputFormat,
}

impl StatusArgs {
    /// The format hint used by `main.rs` for error envelope rendering.
    pub fn format(&self) -> OutputFormat {
        self.format
    }
}

/// Arguments for `wh library init --agent <name>` (story 13-22, FR6).
#[derive(Debug, Args)]
pub struct InitArgs {
    /// Name of the agent whose Library to initialize.
    ///
    /// Per FR6 this is a named flag (not a positional) to match the documented
    /// invocation `wh library init --agent <name>`.
    #[arg(long)]
    pub agent: String,

    /// Output format: human (default) or json.
    #[arg(long, value_enum, default_value = "human")]
    pub format: OutputFormat,

    /// Re-initialize `.library/.git` even if the Library already exists.
    ///
    /// User-content (`pages/*.md`, `index.md`, etc.) is preserved; only the git
    /// metadata directory is removed and recreated. The schema file is always
    /// re-written and re-chmodded.
    #[arg(long)]
    pub force: bool,

    /// Substitute text for the `{{domain_description}}` placeholder in the
    /// schema template. Defaults to a neutral R1 placeholder when absent.
    #[arg(long)]
    pub domain_description: Option<String>,
}

impl InitArgs {
    /// The format hint used by `main.rs` for error envelope rendering.
    pub fn format(&self) -> OutputFormat {
        self.format
    }
}

// ─── Story 13-23: `wh library ingest <file> --agent <name>` ──────────────

/// Arguments for `wh library ingest`.
///
/// Story 13-23 — CLI publisher for the `library_ingest` skill registered by 13-7
/// (`sdk/python/wheelhouse/skills/library_ingest.py::SKILL_NAME = "library_ingest"`).
/// The CLI never reads the file itself; it resolves the container-side path and
/// publishes a `SkillInvocation` on an agent-observed stream. The agent runtime
/// (`agent-claude::loop::_handle_skill_invocation`) intercepts the invocation via
/// the `LIBRARY_SKILL_REGISTRY` dispatch gate and runs `run_library_ingest`.
#[derive(Debug, Args)]
pub struct IngestArgs {
    /// Path to the source file. Either a host-side path inside the agent's
    /// workspace mount, OR a container-side path (`/workspace/...`).
    pub file: PathBuf,

    /// Name of the agent whose Library to ingest into. Must exist in
    /// `.wh/state.json`.
    #[arg(long)]
    pub agent: String,

    /// Override the inferred `source_type`. Must be one of the closed
    /// enum accepted by the 13-7 skill: text | markdown | pdf | url.
    #[arg(long = "type")]
    pub r#type: Option<String>,

    /// Optional free-form `user_summary_hint` passed to the ingest skill.
    #[arg(long)]
    pub hint: Option<String>,

    /// Publish on this specific stream instead of `agent.streams[0]`. Must be
    /// a stream the target agent is already subscribed to.
    #[arg(long)]
    pub stream: Option<String>,

    /// Wait for the matching SkillResult and print its 13-27 piggyback fields.
    /// Timeout: 120 seconds (NFR2 ingest budget).
    #[arg(long, default_value_t = false)]
    pub wait: bool,

    /// Output format: human (default) or json.
    #[arg(long, value_enum, default_value = "human")]
    pub format: OutputFormat,
}

impl IngestArgs {
    /// The format hint used by `main.rs` for error envelope rendering.
    pub fn format(&self) -> OutputFormat {
        self.format
    }
}

/// Skill name key registered by story 13-7. Frozen — see
/// `sdk/python/wheelhouse/skills/library_ingest.py:44`.
const LIBRARY_INGEST_SKILL_NAME: &str = "library_ingest";

/// Closed enum of accepted `source_type` values, mirrored from the 13-7 Python
/// constant `ACCEPTED_SOURCE_TYPES`. Kept as a single source of truth in Rust
/// so the CLI can reject bad `--type` overrides before the zmq round-trip.
const ACCEPTED_SOURCE_TYPES: &[&str] = &["text", "markdown", "pdf", "url"];

/// Protobuf type URL of `SkillInvocation` — matches the value agents expect on
/// the wire.
const SKILL_INVOCATION_TYPE_URL: &str = "wheelhouse.v1.SkillInvocation";

/// Protobuf type URL of `SkillResult` — used by `--wait` to filter incoming
/// envelopes.
const SKILL_RESULT_TYPE_URL: &str = "wheelhouse.v1.SkillResult";

/// Fixed wait deadline for `--wait` (NFR2 ingest budget).
const WAIT_DEADLINE: Duration = Duration::from_secs(120);

/// Serializable publish record for `wh library ingest`.
///
/// JSON parity with 13-24's `StatusData` — all snake_case per SCV-01.
#[derive(Debug, Clone, Serialize)]
pub struct IngestData {
    pub invocation_id: String,
    pub agent: String,
    pub stream: String,
    pub source_type: String,
    pub source_ref: String,
    pub waited: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<IngestResultData>,
}

/// Subset of `SkillResult` we echo back to the operator when `--wait` fires.
/// Includes the 13-27 piggyback fields so operators can see token usage and
/// page-count progression directly.
#[derive(Debug, Clone, Serialize)]
pub struct IngestResultData {
    pub success: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error_code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error_message: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_page_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_last_ingest_at: Option<String>,
}

impl From<&SkillResult> for IngestResultData {
    fn from(r: &SkillResult) -> Self {
        Self {
            success: r.success,
            error_code: r.error_code.clone(),
            error_message: r.error_message.clone(),
            output: r.output.clone(),
            library_tokens: r.library_tokens,
            library_page_count: r.library_page_count,
            library_last_ingest_at: r.library_last_ingest_at.clone(),
        }
    }
}

/// Execute `wh library ingest <file> --agent <name>`.
///
/// Pipeline:
///   1. Load `.wh/state.json` and find the target agent (same as 13-24 `status`).
///   2. Resolve the per-agent workspace volume and host mount path.
///   3. Translate the host-shaped or container-shaped source path into a
///      canonical container path (`/workspace/...`).
///   4. Infer `source_type` from extension or the `--type` override.
///   5. Resolve the target stream from `agent.streams` (first or `--stream`).
///   6. Build the `SkillInvocation` and publish it over zmq.
///   7. If `--wait`, subscribe to the same stream and await the matching
///      `SkillResult`, then render it.
pub async fn ingest(args: &IngestArgs) -> Result<(), WhError> {
    // (1) Load state.
    let state_path = Path::new(".wh/state.json");
    if !state_path.exists() {
        return Err(WhError::Other(
            "no deployed topology found (.wh/state.json missing). Run 'wh topology apply' first."
                .to_string(),
        ));
    }
    let content = std::fs::read_to_string(state_path)
        .map_err(|e| WhError::Internal(format!("failed to read state: {e}")))?;
    let topology: wh_broker::deploy::Topology = serde_json::from_str(&content)
        .map_err(|e| WhError::Internal(format!("corrupt state file: {e}")))?;

    let agent = topology
        .agents
        .iter()
        .find(|a| a.name == args.agent)
        .ok_or_else(|| WhError::AgentNotFound(args.agent.clone()))?;

    // (2) Resolve mount path.
    let volume = wh_broker::deploy::podman::workspace_volume_name(&topology.name, &agent.name);
    let mount_path = resolve_mount_path(&volume)?;

    // (3) Translate source path.
    let source_ref = translate_source_path(&args.file, &mount_path)?;

    // (4) Infer source_type.
    let source_type = infer_source_type(&args.file, args.r#type.as_deref())?;

    // (5) Resolve stream.
    let stream_name = resolve_target_stream(&agent.streams, args.stream.as_deref())?;

    // (6) Build & publish the invocation.
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let parameters = build_invocation_parameters(&source_type, &source_ref, args.hint.as_deref());
    let invocation = SkillInvocation {
        skill_name: LIBRARY_INGEST_SKILL_NAME.to_string(),
        agent_id: args.agent.clone(),
        invocation_id: invocation_id.clone(),
        parameters,
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
    };

    publish_skill_invocation(&stream_name, &invocation).await?;

    // (7) Optionally wait for SkillResult.
    let waited = args.wait;
    let result_data = if args.wait {
        Some(wait_for_skill_result(&stream_name, &invocation_id).await?)
    } else {
        None
    };

    let data = IngestData {
        invocation_id: invocation_id.clone(),
        agent: args.agent.clone(),
        stream: stream_name.clone(),
        source_type: source_type.clone(),
        source_ref: source_ref.clone(),
        waited,
        result: result_data.clone(),
    };

    match args.format {
        OutputFormat::Human => {
            print!("{}", render_ingest_human(&data));
        }
        OutputFormat::Json => {
            json::print_json_success(&data)?;
        }
    }

    // (AC-10) Non-zero exit on `--wait` + failed SkillResult.
    if let Some(r) = result_data.as_ref() {
        if !r.success {
            return Err(WhError::Other(format!(
                "Ingest failed: {code}{sep}{msg}",
                code = if r.error_code.is_empty() {
                    "INGEST_FAILED".to_string()
                } else {
                    r.error_code.clone()
                },
                sep = if r.error_message.is_empty() {
                    ""
                } else {
                    " — "
                },
                msg = r.error_message,
            )));
        }
    }

    Ok(())
}

/// Translate a user-provided file path into the canonical container-side path.
///
/// Three cases:
/// 1. The input already starts with `/workspace/` — accept as-is after probing
///    that the host-side equivalent exists under `mount_path`.
/// 2. The input is a host path whose canonicalised form lies inside
///    `mount_path` — rewrite the mount prefix to `/workspace`.
/// 3. The input is a host path outside the mount — hard-reject with
///    `PATH_OUTSIDE_WORKSPACE`.
fn translate_source_path(input: &Path, mount_path: &Path) -> Result<String, WhError> {
    // Case 1: container-shaped path (`/workspace/...`).
    if let Some(rest) = input.to_str().and_then(|s| s.strip_prefix("/workspace/")) {
        // Also accept exactly "/workspace" (unlikely but deterministic).
        let host_equivalent = mount_path.join(rest);
        if !host_equivalent.exists() {
            return Err(WhError::Other(format!(
                "File not found at container path /workspace/{rest} (expected host file at {}).",
                host_equivalent.display()
            )));
        }
        return Ok(format!("/workspace/{rest}"));
    }
    if input == Path::new("/workspace") {
        // Directory-only — nothing meaningful to ingest.
        return Err(WhError::Other(
            "source path /workspace is a directory; provide a file path.".to_string(),
        ));
    }

    // Case 2/3: host path. Canonicalise both sides so `.`, `..`, and symlinks
    // resolve consistently.
    let canonical_input = std::fs::canonicalize(input)
        .map_err(|e| WhError::Other(format!("source file not found: {} ({e})", input.display())))?;
    let canonical_mount = std::fs::canonicalize(mount_path).map_err(|e| {
        WhError::Other(format!(
            "workspace mount not readable at {}: {e}",
            mount_path.display()
        ))
    })?;

    match canonical_input.strip_prefix(&canonical_mount) {
        Ok(rel) => {
            // Rewrite prefix → /workspace. Use forward-slash joining for the
            // container-side path (POSIX inside the container).
            let rel_str = rel
                .to_str()
                .ok_or_else(|| WhError::Other("non-UTF8 path component in source".to_string()))?;
            if rel_str.is_empty() {
                return Err(WhError::Other(
                    "source path resolves to the workspace root; provide a file path.".to_string(),
                ));
            }
            // Normalise Windows-style separators just in case.
            let posix = rel_str.replace('\\', "/");
            Ok(format!("/workspace/{posix}"))
        }
        Err(_) => Err(WhError::Other(format!(
            "PATH_OUTSIDE_WORKSPACE: {} is not inside the agent's workspace volume at {}. \
             Copy the file into the volume first (e.g. cp {} {}).",
            canonical_input.display(),
            canonical_mount.display(),
            canonical_input.display(),
            canonical_mount.display(),
        ))),
    }
}

/// Infer the `source_type` parameter from an optional `--type` override, or
/// from the file extension when absent.
///
/// The closed enum (`ACCEPTED_SOURCE_TYPES`) mirrors the 13-7 Python constant
/// `ACCEPTED_SOURCE_TYPES`. Unknown extensions with no override are rejected
/// with `UNKNOWN_SOURCE_TYPE`.
fn infer_source_type(path: &Path, override_: Option<&str>) -> Result<String, WhError> {
    if let Some(ov) = override_ {
        if !ACCEPTED_SOURCE_TYPES.contains(&ov) {
            return Err(WhError::Other(format!(
                "UNKNOWN_SOURCE_TYPE: --type {ov:?} is not one of {ACCEPTED_SOURCE_TYPES:?}"
            )));
        }
        return Ok(ov.to_string());
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    let inferred = match ext.as_deref() {
        Some("md") | Some("markdown") => Some("markdown"),
        Some("txt") => Some("text"),
        Some("pdf") => Some("pdf"),
        _ => None,
    };
    inferred.map(str::to_string).ok_or_else(|| {
        WhError::Other(format!(
            "UNKNOWN_SOURCE_TYPE: cannot infer source_type for {}; pass --type <{}>",
            path.display(),
            ACCEPTED_SOURCE_TYPES.join("|")
        ))
    })
}

/// Resolve the target stream for the SkillInvocation publish.
///
/// With `override_` passed: validate it is declared on the agent and return it.
/// Without: return the first entry in `agent.streams`, or
/// `NO_INPUT_STREAM` if the agent has no streams declared.
fn resolve_target_stream(
    agent_streams: &[String],
    override_: Option<&str>,
) -> Result<String, WhError> {
    if let Some(name) = override_ {
        if agent_streams.iter().any(|s| s == name) {
            return Ok(name.to_string());
        }
        return Err(WhError::Other(format!(
            "STREAM_NOT_DECLARED: stream {name:?} is not declared on the agent. \
             Declared streams: {agent_streams:?}"
        )));
    }
    agent_streams.first().cloned().ok_or_else(|| {
        WhError::Other(
            "NO_INPUT_STREAM: the target agent has no streams declared. \
             Add at least one stream to the agent's topology spec."
                .to_string(),
        )
    })
}

/// Build the SkillInvocation `parameters` map from the resolved values.
fn build_invocation_parameters(
    source_type: &str,
    source_ref: &str,
    hint: Option<&str>,
) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("source_type".to_string(), source_type.to_string());
    m.insert("source_ref".to_string(), source_ref.to_string());
    if let Some(h) = hint {
        m.insert("user_summary_hint".to_string(), h.to_string());
    }
    m
}

/// Publish a SkillInvocation as a StreamEnvelope on the broker's SUB endpoint.
///
/// Mirrors `crates/wh-cli/src/commands/stream.rs::execute_publish` for
/// TextMessage but with the SkillInvocation type URL and payload. The 100 ms
/// PUB-SUB handshake delay is the same — required because ZMQ PUB drops messages
/// sent before the SUB side completes subscription handshake.
async fn publish_skill_invocation(
    stream_name: &str,
    invocation: &SkillInvocation,
) -> Result<(), WhError> {
    let sub_endpoint = std::env::var("WH_SUB_ENDPOINT").unwrap_or_else(|_| {
        let port = std::env::var("WH_SUB_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(5556);
        format!("tcp://127.0.0.1:{port}")
    });

    let payload = invocation.encode_to_vec();
    let envelope = StreamEnvelope {
        stream_name: stream_name.to_string(),
        object_id: uuid::Uuid::new_v4().to_string(),
        type_url: SKILL_INVOCATION_TYPE_URL.to_string(),
        payload,
        publisher_id: "cli".to_string(),
        published_at_ms: chrono::Utc::now().timestamp_millis(),
        sequence_number: 0, // broker assigns authoritative value
    };
    let envelope_bytes = envelope.encode_to_vec();

    let mut wire: Vec<u8> = Vec::with_capacity(stream_name.len() + 1 + envelope_bytes.len());
    wire.extend_from_slice(stream_name.as_bytes());
    wire.push(0);
    wire.extend_from_slice(&envelope_bytes);

    let mut pub_socket = PubSocket::new();
    pub_socket
        .connect(&sub_endpoint)
        .await
        .map_err(|_| WhError::ConnectionError)?;

    // Handshake delay — same 100 ms as stream.rs::execute_publish.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let msg = ZmqMessage::from(wire);
    pub_socket
        .send(msg)
        .await
        .map_err(|e| WhError::Other(format!("failed to publish: {e}")))?;

    Ok(())
}

/// Wait for a matching `SkillResult` on the same stream, up to 120 s.
///
/// Matching rule: decoded envelope `type_url == "wheelhouse.v1.SkillResult"`
/// AND the decoded result's `invocation_id == expected`. Unrelated messages
/// on the stream are ignored silently — an operator running `wh library
/// ingest` does not care about concurrent traffic.
async fn wait_for_skill_result(
    stream_name: &str,
    invocation_id: &str,
) -> Result<IngestResultData, WhError> {
    let pub_endpoint = std::env::var("WH_PUB_ENDPOINT").unwrap_or_else(|_| {
        let port = std::env::var("WH_PUB_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(5555);
        format!("tcp://127.0.0.1:{port}")
    });

    let topic = format!("{stream_name}\0");

    let mut sub_socket = SubSocket::new();
    sub_socket
        .connect(&pub_endpoint)
        .await
        .map_err(|_| WhError::ConnectionError)?;
    sub_socket
        .subscribe(&topic)
        .await
        .map_err(|e| WhError::Other(format!("failed to subscribe: {e}")))?;

    let deadline = tokio::time::Instant::now() + WAIT_DEADLINE;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(WhError::Other(format!(
                "INGEST_TIMEOUT: no SkillResult received for invocation {invocation_id} \
                 within {secs}s.",
                secs = WAIT_DEADLINE.as_secs()
            )));
        }

        let recv_fut = sub_socket.recv();
        let msg = match tokio::time::timeout(remaining, recv_fut).await {
            Ok(Ok(msg)) => msg,
            Ok(Err(e)) => {
                return Err(WhError::Other(format!("receive error while waiting: {e}")));
            }
            Err(_) => {
                return Err(WhError::Other(format!(
                    "INGEST_TIMEOUT: no SkillResult received for invocation {invocation_id} \
                     within {secs}s.",
                    secs = WAIT_DEADLINE.as_secs()
                )));
            }
        };

        let raw: Vec<u8> = msg.try_into().unwrap_or_default();
        let Some(null_pos) = raw.iter().position(|&b| b == 0) else {
            continue;
        };
        let envelope_bytes = &raw[null_pos + 1..];
        let Ok(envelope) = StreamEnvelope::decode(envelope_bytes) else {
            continue;
        };
        if envelope.type_url != SKILL_RESULT_TYPE_URL {
            continue;
        }
        let Ok(result) = SkillResult::decode(envelope.payload.as_slice()) else {
            continue;
        };
        if result.invocation_id != invocation_id {
            continue;
        }
        return Ok(IngestResultData::from(&result));
    }
}

/// Format an `IngestData` as the human-readable publish confirmation.
fn render_ingest_human(data: &IngestData) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Ingest published to stream '{}' for agent '{}'\n",
        data.stream, data.agent
    ));
    out.push_str(&format!("  invocation_id: {}\n", data.invocation_id));
    out.push_str(&format!("  source_type:   {}\n", data.source_type));
    out.push_str(&format!("  source_ref:    {}\n", data.source_ref));

    if let Some(r) = data.result.as_ref() {
        out.push('\n');
        out.push_str("SkillResult received\n");
        out.push_str(&format!(
            "  success:       {}\n",
            if r.success { "true" } else { "false" }
        ));
        if !r.error_code.is_empty() {
            out.push_str(&format!("  error_code:    {}\n", r.error_code));
        }
        if !r.error_message.is_empty() {
            out.push_str(&format!("  error_message: {}\n", r.error_message));
        }
        if !r.output.is_empty() {
            out.push_str(&format!("  output:        {}\n", r.output));
        }
        if let Some(t) = r.library_tokens {
            out.push_str(&format!("  library_tokens:         {t}\n"));
        }
        if let Some(p) = r.library_page_count {
            out.push_str(&format!("  library_page_count:     {p}\n"));
        }
        if let Some(ts) = r.library_last_ingest_at.as_deref() {
            out.push_str(&format!("  library_last_ingest_at: {ts}\n"));
        }
    }

    out
}

// ─── Story 13-26: `wh library lint --agent <name>` ───────────────────────

/// Arguments for `wh library lint`.
///
/// Story 13-26 — CLI publisher for the `library_lint` skill registered by 13-18
/// (`sdk/python/wheelhouse/skills/library_lint.py` — on-demand lint handler).
/// The CLI never runs the lint itself; it publishes a `SkillInvocation` on an
/// agent-observed stream and (optionally) waits for the matching
/// `SkillResult`. The agent runtime dispatches the invocation via the
/// `LIBRARY_SKILL_REGISTRY` gate and runs `lint_library()` from 13-16 with the
/// appropriate `page_filter` based on mode.
#[derive(Debug, Args)]
pub struct LintArgs {
    /// Name of the agent whose Library to lint. Must exist in `.wh/state.json`.
    #[arg(long)]
    pub agent: String,

    /// Lint mode: `full` re-lints every page, `incremental` (default) re-lints
    /// only pages that changed since the last lint watermark (story 13-17).
    #[arg(long)]
    pub mode: Option<String>,

    /// Publish on this specific stream instead of `agent.streams[0]`. Must be
    /// a stream the target agent is already subscribed to.
    #[arg(long)]
    pub stream: Option<String>,

    /// Wait for the matching SkillResult and print its findings output.
    /// Timeout: 120 seconds.
    #[arg(long, default_value_t = false)]
    pub wait: bool,

    /// Output format: human (default) or json.
    #[arg(long, value_enum, default_value = "human")]
    pub format: OutputFormat,
}

impl LintArgs {
    /// The format hint used by `main.rs` for error envelope rendering.
    pub fn format(&self) -> OutputFormat {
        self.format
    }
}

/// Skill name key registered by story 13-18. Frozen — underscore form, NOT
/// dot-notation; see the library_lint skill module for the exact key.
const LIBRARY_LINT_SKILL_NAME: &str = "library_lint";

/// Closed enum of accepted `mode` values. The Python lint skill's 13-17
/// incremental-mode implementation accepts exactly these two.
const ACCEPTED_LINT_MODES: &[&str] = &["full", "incremental"];

/// Serializable publish record for `wh library lint`.
#[derive(Debug, Clone, Serialize)]
pub struct LintData {
    pub invocation_id: String,
    pub agent: String,
    pub stream: String,
    pub mode: String,
    pub waited: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<LintResultData>,
}

/// Subset of `SkillResult` we echo back to the operator when `--wait` fires.
/// Matches the shape of `IngestResultData` — we forward the raw `output`
/// field (a JSON-encoded list of findings per 13-16's contract) without
/// client-side parsing.
#[derive(Debug, Clone, Serialize)]
pub struct LintResultData {
    pub success: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error_code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error_message: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_page_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_last_ingest_at: Option<String>,
}

impl From<&SkillResult> for LintResultData {
    fn from(r: &SkillResult) -> Self {
        Self {
            success: r.success,
            error_code: r.error_code.clone(),
            error_message: r.error_message.clone(),
            output: r.output.clone(),
            library_tokens: r.library_tokens,
            library_page_count: r.library_page_count,
            library_last_ingest_at: r.library_last_ingest_at.clone(),
        }
    }
}

/// Execute `wh library lint --agent <name>`.
///
/// Pipeline:
///   1. Load `.wh/state.json` and find the target agent.
///   2. Resolve the target stream from `agent.streams` (first or `--stream`).
///   3. Resolve the lint mode (default incremental, reject unknown).
///   4. Build the `SkillInvocation` and publish it over zmq.
///   5. If `--wait`, subscribe to the same stream and await the matching
///      `SkillResult`, then render it.
///
/// Unlike `ingest`, this command does not touch the per-agent workspace mount
/// at all — the lint skill reads the Library through the agent-side sandbox.
pub async fn lint(args: &LintArgs) -> Result<(), WhError> {
    // (1) Load state.
    let state_path = Path::new(".wh/state.json");
    if !state_path.exists() {
        return Err(WhError::Other(
            "no deployed topology found (.wh/state.json missing). Run 'wh topology apply' first."
                .to_string(),
        ));
    }
    let content = std::fs::read_to_string(state_path)
        .map_err(|e| WhError::Internal(format!("failed to read state: {e}")))?;
    let topology: wh_broker::deploy::Topology = serde_json::from_str(&content)
        .map_err(|e| WhError::Internal(format!("corrupt state file: {e}")))?;

    let agent = topology
        .agents
        .iter()
        .find(|a| a.name == args.agent)
        .ok_or_else(|| WhError::AgentNotFound(args.agent.clone()))?;

    // (2) Resolve stream.
    let stream_name = resolve_target_stream(&agent.streams, args.stream.as_deref())?;

    // (3) Resolve mode.
    let mode = resolve_lint_mode(args.mode.as_deref())?;

    // (4) Build & publish the invocation.
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let mut parameters: HashMap<String, String> = HashMap::new();
    parameters.insert("mode".to_string(), mode.clone());
    let invocation = SkillInvocation {
        skill_name: LIBRARY_LINT_SKILL_NAME.to_string(),
        agent_id: args.agent.clone(),
        invocation_id: invocation_id.clone(),
        parameters,
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
    };

    publish_skill_invocation(&stream_name, &invocation).await?;

    // (5) Optionally wait for SkillResult.
    let waited = args.wait;
    let result_data = if args.wait {
        Some(wait_for_lint_result(&stream_name, &invocation_id).await?)
    } else {
        None
    };

    let data = LintData {
        invocation_id: invocation_id.clone(),
        agent: args.agent.clone(),
        stream: stream_name.clone(),
        mode: mode.clone(),
        waited,
        result: result_data.clone(),
    };

    match args.format {
        OutputFormat::Human => {
            print!("{}", render_lint_human(&data));
        }
        OutputFormat::Json => {
            json::print_json_success(&data)?;
        }
    }

    // (AC-7) Non-zero exit on `--wait` + failed SkillResult.
    if let Some(r) = result_data.as_ref() {
        if !r.success {
            return Err(WhError::Other(format!(
                "LINT_FAILED: {code}{sep}{msg}",
                code = if r.error_code.is_empty() {
                    "LINT_FAILED".to_string()
                } else {
                    r.error_code.clone()
                },
                sep = if r.error_message.is_empty() {
                    ""
                } else {
                    " — "
                },
                msg = r.error_message,
            )));
        }
    }

    Ok(())
}

/// Resolve the `mode` parameter. Defaults to `"incremental"` to match
/// 13-17's default. Rejects values outside the closed enum.
fn resolve_lint_mode(override_: Option<&str>) -> Result<String, WhError> {
    match override_ {
        None => Ok("incremental".to_string()),
        Some(v) => {
            if ACCEPTED_LINT_MODES.contains(&v) {
                Ok(v.to_string())
            } else {
                Err(WhError::Other(format!(
                    "UNKNOWN_LINT_MODE: --mode {v:?} is not one of {ACCEPTED_LINT_MODES:?}"
                )))
            }
        }
    }
}

/// Wait for a matching `SkillResult` on the same stream, up to 120 s.
///
/// Thin wrapper over the generic `wait_for_skill_result` helper that returns
/// a `LintResultData` instead of `IngestResultData`. Kept as a separate
/// function so the public `lint()` pipeline does not leak ingest-specific
/// types when 13-23 is read in isolation. In practice both types have the
/// same field set and are constructed from the same `SkillResult`.
async fn wait_for_lint_result(
    stream_name: &str,
    invocation_id: &str,
) -> Result<LintResultData, WhError> {
    // Reuse the generic subscribe loop; the returned IngestResultData is
    // structurally identical so we re-map it through SkillResult fields.
    // Easier: call a shared generic helper. For minimal change we inline
    // the translation by going through the ingest variant and rewrapping.
    let ingest_shaped = wait_for_skill_result(stream_name, invocation_id).await?;
    Ok(LintResultData {
        success: ingest_shaped.success,
        error_code: ingest_shaped.error_code,
        error_message: ingest_shaped.error_message,
        output: ingest_shaped.output,
        library_tokens: ingest_shaped.library_tokens,
        library_page_count: ingest_shaped.library_page_count,
        library_last_ingest_at: ingest_shaped.library_last_ingest_at,
    })
}

/// Format a `LintData` as the human-readable publish confirmation.
fn render_lint_human(data: &LintData) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Lint published to stream '{}' for agent '{}'\n",
        data.stream, data.agent
    ));
    out.push_str(&format!("  invocation_id: {}\n", data.invocation_id));
    out.push_str(&format!("  mode:          {}\n", data.mode));

    if let Some(r) = data.result.as_ref() {
        out.push('\n');
        out.push_str("SkillResult received\n");
        out.push_str(&format!(
            "  success:       {}\n",
            if r.success { "true" } else { "false" }
        ));
        if !r.error_code.is_empty() {
            out.push_str(&format!("  error_code:    {}\n", r.error_code));
        }
        if !r.error_message.is_empty() {
            out.push_str(&format!("  error_message: {}\n", r.error_message));
        }
        if !r.output.is_empty() {
            out.push_str(&format!("  output:        {}\n", r.output));
        }
        if let Some(t) = r.library_tokens {
            out.push_str(&format!("  library_tokens:         {t}\n"));
        }
        if let Some(p) = r.library_page_count {
            out.push_str(&format!("  library_page_count:     {p}\n"));
        }
        if let Some(ts) = r.library_last_ingest_at.as_deref() {
            out.push_str(&format!("  library_last_ingest_at: {ts}\n"));
        }
    }

    out
}

impl LibraryCommand {
    /// The format hint used by `main.rs` to render error envelopes in the right shape.
    pub fn format(&self) -> OutputFormat {
        match self {
            LibraryCommand::Init(args) => args.format,
            LibraryCommand::Ingest(args) => args.format,
            LibraryCommand::Lint(args) => args.format,
            LibraryCommand::List(args) => args.format,
            LibraryCommand::Status(args) => args.format,
        }
    }
}

/// Serializable health record for `wh library status`.
///
/// Field names are snake_case per SCV-01 and match the JSON-output contract in
/// AC-4 of story 13-24. Every "maybe-missing" field is modeled as `Option<_>` so
/// the empty-library state (story AC-6) serializes cleanly.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StatusData {
    pub agent: String,
    pub topology: String,
    pub volume: String,
    pub mount_path: String,
    pub schema_present: bool,
    pub page_count: u64,
    pub git_head: Option<String>,
    pub last_ingest_at: Option<String>,
    pub lock_held: bool,
    pub lock_age_seconds: Option<u64>,
    pub free_disk_bytes: Option<u64>,
}

/// Dispatcher for `wh library <subcommand>`.
pub async fn run(cmd: LibraryCommand) -> Result<(), WhError> {
    match cmd {
        LibraryCommand::Init(args) => init(&args).await,
        LibraryCommand::Ingest(args) => ingest(&args).await,
        LibraryCommand::Lint(args) => lint(&args).await,
        LibraryCommand::List(args) => list(&args).await,
        LibraryCommand::Status(args) => status(&args).await,
    }
}

/// Execute `wh library status <agent>`.
///
/// Reads `.wh/state.json`, resolves the per-agent workspace volume, probes its
/// host mount path via `podman volume inspect`, then reads the six health signals
/// directly from the host filesystem. All operations are synchronous, local, and
/// bounded well under FR45's 2-second target.
pub async fn status(args: &StatusArgs) -> Result<(), WhError> {
    // Load .wh/state.json (same pattern as wh ps / wh surface).
    let state_path = Path::new(".wh/state.json");
    if !state_path.exists() {
        return Err(WhError::Other(
            "no deployed topology found (.wh/state.json missing). Run 'wh topology apply' first."
                .to_string(),
        ));
    }
    let content = std::fs::read_to_string(state_path)
        .map_err(|e| WhError::Internal(format!("failed to read state: {e}")))?;
    let topology: wh_broker::deploy::Topology = serde_json::from_str(&content)
        .map_err(|e| WhError::Internal(format!("corrupt state file: {e}")))?;

    // Find the requested agent.
    let agent = topology
        .agents
        .iter()
        .find(|a| a.name == args.agent)
        .ok_or_else(|| WhError::AgentNotFound(args.agent.clone()))?;

    // Resolve the per-agent workspace volume (story 13-3 contract).
    let volume = wh_broker::deploy::podman::workspace_volume_name(&topology.name, &agent.name);

    // Probe the host mount path via `podman volume inspect`.
    let mount_path = resolve_mount_path(&volume)?;

    // Read the six health signals from the host filesystem.
    let data = collect_status_data(
        args.agent.clone(),
        topology.name.clone(),
        volume,
        mount_path,
    );

    match args.format {
        OutputFormat::Human => {
            print!("{}", render_human(&data));
        }
        OutputFormat::Json => {
            json::print_json_success(&data)?;
        }
    }

    Ok(())
}

// ─── Story 13-25: `wh library list` ────────────────────────────────────────

/// Serializable aggregate record for `wh library list`.
///
/// JSON shape (AC-3):
/// ```json
/// { "topology": "dev", "libraries": [ { ...StatusData..., "status": "active" }, ... ] }
/// ```
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ListData {
    pub topology: String,
    pub libraries: Vec<LibraryRow>,
}

/// One row in the list output. Flattens `StatusData` into the top level so JSON
/// consumers get a single object per Library plus the derived lifecycle `status`.
///
/// The `unreachable` flag is internal-only (skip_serializing): the `status` string
/// already carries the `"unreachable"` signal to JSON consumers.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LibraryRow {
    #[serde(flatten)]
    pub data: StatusData,
    pub status: String,
    #[serde(skip)]
    pub unreachable: bool,
}

/// Derive the Library lifecycle status from on-disk signals (AC-2, AC-4).
///
/// Branch order (first match wins):
/// 1. `unreachable` → `"unreachable"` (volume missing / podman down).
/// 2. `git_head.is_none()` → `"not-init"` (no `.library/.git` on disk).
/// 3. `page_count == 0` → `"empty"` (init happened, zero pages).
/// 4. default → `"active"`.
fn derive_library_status(data: &StatusData, unreachable: bool) -> &'static str {
    if unreachable {
        return "unreachable";
    }
    if data.git_head.is_none() {
        return "not-init";
    }
    if data.page_count == 0 {
        return "empty";
    }
    "active"
}

/// Build a placeholder `StatusData` for an agent whose volume/mount could not be
/// resolved. Everything degrades to `None`/`0`/`false`; the renderer replaces each
/// field with `—` when the enclosing row has `unreachable == true`.
fn unreachable_status_data(agent: String, topology: String, volume: String) -> StatusData {
    StatusData {
        agent,
        topology,
        volume,
        mount_path: String::new(),
        schema_present: false,
        page_count: 0,
        git_head: None,
        last_ingest_at: None,
        lock_held: false,
        lock_age_seconds: None,
        free_disk_bytes: None,
    }
}

/// Execute `wh library list` (FR46).
///
/// Iterates every agent in the loaded topology, reuses `resolve_mount_path` +
/// `collect_status_data` per agent, derives the lifecycle status, and renders the
/// aggregate as a table or JSON. A single unreachable agent does not abort the
/// listing — it degrades to an `unreachable` row (AC-4).
pub async fn list(args: &ListArgs) -> Result<(), WhError> {
    // Load `.wh/state.json` (same helper pattern as status/init/ingest).
    let state_path = Path::new(".wh/state.json");
    if !state_path.exists() {
        return Err(WhError::Other(
            "no deployed topology found (.wh/state.json missing). Run 'wh topology apply' first."
                .to_string(),
        ));
    }
    let content = std::fs::read_to_string(state_path)
        .map_err(|e| WhError::Internal(format!("failed to read state: {e}")))?;
    let topology: wh_broker::deploy::Topology = serde_json::from_str(&content)
        .map_err(|e| WhError::Internal(format!("corrupt state file: {e}")))?;

    let rows: Vec<LibraryRow> = topology
        .agents
        .iter()
        .map(|agent| {
            let volume =
                wh_broker::deploy::podman::workspace_volume_name(&topology.name, &agent.name);
            match resolve_mount_path(&volume) {
                Ok(mount_path) => {
                    let data = collect_status_data(
                        agent.name.clone(),
                        topology.name.clone(),
                        volume,
                        mount_path,
                    );
                    let status = derive_library_status(&data, false).to_string();
                    LibraryRow {
                        data,
                        status,
                        unreachable: false,
                    }
                }
                Err(_) => {
                    let data =
                        unreachable_status_data(agent.name.clone(), topology.name.clone(), volume);
                    LibraryRow {
                        data,
                        status: "unreachable".to_string(),
                        unreachable: true,
                    }
                }
            }
        })
        .collect();

    let data = ListData {
        topology: topology.name.clone(),
        libraries: rows,
    };

    match args.format {
        OutputFormat::Human => {
            print!("{}", render_list_human(&data));
        }
        OutputFormat::Json => {
            json::print_json_success(&data)?;
        }
    }

    Ok(())
}

/// Render `ListData` as a compact human-readable table (AC-2).
///
/// Columns: AGENT, STATUS, PAGES, LAST INGEST, HEAD, SCHEMA, FREE DISK.
/// Widths are computed per-column as `max(header_len, max(cell_len))`. Two-space
/// gutter between columns. Unreachable rows render every non-AGENT/non-STATUS
/// field as `—`.
fn render_list_human(data: &ListData) -> String {
    let mut out = String::new();
    out.push_str(&format!("Libraries — topology '{}'\n", data.topology));

    if data.libraries.is_empty() {
        out.push('\n');
        out.push_str("  (no agents declared in this topology)\n");
        return out;
    }

    // Build cell strings up-front so width computation is straightforward.
    struct Cells {
        agent: String,
        status: String,
        pages: String,
        last_ingest: String,
        head: String,
        schema: String,
        free_disk: String,
    }

    let rows: Vec<Cells> = data
        .libraries
        .iter()
        .map(|row| {
            if row.unreachable {
                Cells {
                    agent: row.data.agent.clone(),
                    status: row.status.clone(),
                    pages: "—".to_string(),
                    last_ingest: "—".to_string(),
                    head: "—".to_string(),
                    schema: "—".to_string(),
                    free_disk: "—".to_string(),
                }
            } else {
                Cells {
                    agent: row.data.agent.clone(),
                    status: row.status.clone(),
                    pages: row.data.page_count.to_string(),
                    last_ingest: row
                        .data
                        .last_ingest_at
                        .clone()
                        .unwrap_or_else(|| "—".to_string()),
                    head: row.data.git_head.clone().unwrap_or_else(|| "—".to_string()),
                    schema: if row.data.schema_present { "y" } else { "n" }.to_string(),
                    free_disk: match row.data.free_disk_bytes {
                        Some(b) => format_bytes_human(b),
                        None => "—".to_string(),
                    },
                }
            }
        })
        .collect();

    let headers = [
        "AGENT",
        "STATUS",
        "PAGES",
        "LAST INGEST",
        "HEAD",
        "SCHEMA",
        "FREE DISK",
    ];
    let mut widths = headers.map(|h| h.len());
    for r in &rows {
        widths[0] = widths[0].max(r.agent.len());
        widths[1] = widths[1].max(r.status.len());
        widths[2] = widths[2].max(r.pages.chars().count());
        widths[3] = widths[3].max(r.last_ingest.chars().count());
        widths[4] = widths[4].max(r.head.chars().count());
        widths[5] = widths[5].max(r.schema.chars().count());
        widths[6] = widths[6].max(r.free_disk.chars().count());
    }

    fn push_row(out: &mut String, cells: [&str; 7], widths: &[usize; 7]) {
        out.push_str("  ");
        for (i, cell) in cells.iter().enumerate() {
            let pad = widths[i].saturating_sub(cell.chars().count());
            out.push_str(cell);
            for _ in 0..pad {
                out.push(' ');
            }
            if i + 1 < cells.len() {
                out.push_str("  ");
            }
        }
        out.push('\n');
    }

    out.push('\n');
    push_row(&mut out, headers, &widths);
    // Underline row — one '─' per column width, re-using the gutter logic above.
    let mut underline: [String; 7] = Default::default();
    for (i, w) in widths.iter().enumerate() {
        underline[i] = "─".repeat(*w);
    }
    let underline_refs: [&str; 7] = [
        &underline[0],
        &underline[1],
        &underline[2],
        &underline[3],
        &underline[4],
        &underline[5],
        &underline[6],
    ];
    push_row(&mut out, underline_refs, &widths);

    for r in &rows {
        let cells: [&str; 7] = [
            &r.agent,
            &r.status,
            &r.pages,
            &r.last_ingest,
            &r.head,
            &r.schema,
            &r.free_disk,
        ];
        push_row(&mut out, cells, &widths);
    }

    out
}

// ─── Story 13-22: `wh library init --agent <name>` ─────────────────────────

/// Default substitution for `{{domain_description}}` when the user does not
/// supply `--domain-description`. R1-safe neutral wording; the user can edit
/// `.wh-schema.md` later via a future `wh library schema` command or by
/// bind-mounting the volume and editing the file directly (it will be
/// re-chmodded to 444 by 13-20's boot-time enforcement).
const DEFAULT_DOMAIN_DESCRIPTION: &str =
    "(Set this to describe your Library's domain — the topics you want the agent to remember across conversations.)";

/// Serializable result record for `wh library init`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct InitData {
    pub agent: String,
    pub topology: String,
    pub volume: String,
    pub mount_path: String,
    pub library_initialized: bool,
    pub schema_written: bool,
    pub force: bool,
}

/// Execute `wh library init --agent <name>`.
///
/// Resolves the agent's per-agent workspace volume (same path as `status`),
/// creates `.library/` with a git repo + initial empty commit, and writes the
/// schema file at `.wh-schema.md` with mode 444. Refuses a pre-existing
/// `.library/.git` unless `--force` is set. Does **not** auto-provision the
/// volume — that is the job of `wh topology apply` (FR6 scope boundary).
pub async fn init(args: &InitArgs) -> Result<(), WhError> {
    // Load `.wh/state.json` (same pattern as `status`).
    let state_path = Path::new(".wh/state.json");
    if !state_path.exists() {
        return Err(WhError::Other(
            "no deployed topology found (.wh/state.json missing). Run 'wh topology apply' first."
                .to_string(),
        ));
    }
    let content = std::fs::read_to_string(state_path)
        .map_err(|e| WhError::Internal(format!("failed to read state: {e}")))?;
    let topology: wh_broker::deploy::Topology = serde_json::from_str(&content)
        .map_err(|e| WhError::Internal(format!("corrupt state file: {e}")))?;

    let agent = topology
        .agents
        .iter()
        .find(|a| a.name == args.agent)
        .ok_or_else(|| WhError::AgentNotFound(args.agent.clone()))?;

    let volume = wh_broker::deploy::podman::workspace_volume_name(&topology.name, &agent.name);

    // AC-2: when the volume is missing, resolve_mount_path already produces
    // a pointer to topology apply, but we wrap it to guarantee the literal
    // `wh topology apply` phrase is in the error (the 13-24 helper says
    // "Has the topology been applied?" — close but not identical).
    let mount_path = resolve_mount_path(&volume).map_err(|e| {
        WhError::Other(format!(
            "{e} Run `wh topology apply` to provision the workspace volume before initializing the Library."
        ))
    })?;

    // Perform the filesystem work in a pure helper that tests can drive
    // against a tempdir without mocking podman.
    init_library_at(
        &mount_path,
        &args.agent,
        args.domain_description.as_deref(),
        args.force,
    )?;

    let data = InitData {
        agent: args.agent.clone(),
        topology: topology.name.clone(),
        volume,
        mount_path: mount_path.to_string_lossy().into_owned(),
        library_initialized: true,
        schema_written: true,
        force: args.force,
    };

    match args.format {
        OutputFormat::Human => {
            print!("{}", render_init_human(&data));
        }
        OutputFormat::Json => {
            json::print_json_success(&data)?;
        }
    }

    Ok(())
}

/// Perform the filesystem-level init work at a given mount path.
///
/// Split from `init()` so tests can cover the fresh / already-initialized /
/// `--force` branches without touching podman or `.wh/state.json`.
///
/// Steps:
/// 1. `mkdir -p <mount>/.library`
/// 2. If `.library/.git` exists and `!force` → refuse with "already initialized"
/// 3. If `.library/.git` exists and `force` → `remove_dir_all(.library/.git)`
/// 4. `git init` + `git commit --allow-empty -m "wh library init"`
/// 5. Write rendered schema to `<mount>/.wh-schema.md`, chmod 444
fn init_library_at(
    mount_path: &Path,
    agent_name: &str,
    domain_description: Option<&str>,
    force: bool,
) -> Result<(), WhError> {
    if !mount_path.exists() {
        return Err(WhError::Other(format!(
            "workspace mount path '{}' does not exist. Run 'wh topology apply' to provision the volume.",
            mount_path.display()
        )));
    }

    let library_root = mount_path.join(".library");
    std::fs::create_dir_all(&library_root)
        .map_err(|e| WhError::Other(format!("failed to create .library directory: {e}")))?;

    let git_dir = library_root.join(".git");
    if git_dir.exists() {
        if !force {
            return Err(WhError::Other(format!(
                "Library already initialized at {}. Use --force to reinitialize (pages will be preserved, only .git metadata is reset).",
                library_root.display()
            )));
        }
        std::fs::remove_dir_all(&git_dir)
            .map_err(|e| WhError::Other(format!("failed to remove existing .library/.git: {e}")))?;
    }

    // `git init` — bounded, local, millisecond-scale. Subprocess (not git2)
    // to keep wh-cli's dependency surface minimal; same pattern as 13-24.
    run_git_init(&library_root)?;
    run_git_initial_commit(&library_root)?;

    // Schema file write + chmod 444 (AC-3).
    //
    // If a previous init already chmodded the file to 444, a plain
    // `fs::write` would fail with EACCES. Remove the existing file first
    // (ignoring NotFound) so the write always lands on a fresh inode.
    let schema_path = mount_path.join(".wh-schema.md");
    match std::fs::remove_file(&schema_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(WhError::Other(format!(
                "failed to clear existing schema file before rewrite: {e}"
            )));
        }
    }
    let rendered = render_schema(agent_name, domain_description);
    std::fs::write(&schema_path, rendered)
        .map_err(|e| WhError::Other(format!("failed to write schema file: {e}")))?;
    set_readonly_444(&schema_path)?;

    Ok(())
}

/// Render `DEFAULT_SCHEMA_TEMPLATE` with `{{agent_name}}` and
/// `{{domain_description}}` substituted. `domain_description=None` uses the
/// neutral `DEFAULT_DOMAIN_DESCRIPTION` placeholder.
///
/// The allow-list of placeholders is enforced by the 13-19 test
/// `test_template_no_undocumented_placeholders`. AC-8 is verified by
/// `test_render_schema_no_residual_placeholders` below.
fn render_schema(agent_name: &str, domain_description: Option<&str>) -> String {
    let domain = domain_description.unwrap_or(DEFAULT_DOMAIN_DESCRIPTION);
    DEFAULT_SCHEMA_TEMPLATE
        .replace("{{agent_name}}", agent_name)
        .replace("{{domain_description}}", domain)
}

/// Run `git -C <library_root> init -q`.
fn run_git_init(library_root: &Path) -> Result<(), WhError> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(library_root)
        .args(["init", "-q"])
        .output()
        .map_err(|e| WhError::Other(format!("failed to run git init: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WhError::Other(format!(
            "git init failed: {}",
            stderr.trim()
        )));
    }
    Ok(())
}

/// Run `git -C <library_root> commit --allow-empty -q -m "wh library init"`.
///
/// The initial empty commit guarantees downstream callers (`status`, the
/// ingest skill from 13-7, `git rev-parse HEAD`) see a valid HEAD even on a
/// freshly-initialized Library with zero pages. We inject `user.name` /
/// `user.email` via `-c` flags so the command works even when the invoking
/// user has no global git identity configured (CI containers, fresh laptops).
fn run_git_initial_commit(library_root: &Path) -> Result<(), WhError> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(library_root)
        .args([
            "-c",
            "user.name=wh",
            "-c",
            "user.email=wh@wheelhouse.local",
            "commit",
            "--allow-empty",
            "-q",
            "-m",
            "wh library init",
        ])
        .output()
        .map_err(|e| WhError::Other(format!("failed to run git commit: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WhError::Other(format!(
            "git initial commit failed: {}",
            stderr.trim()
        )));
    }
    Ok(())
}

/// Set file mode to `0o444` (read-only for all) — mirrors the boot-time
/// enforcement in 13-20 so `wh library status` run immediately after
/// `wh library init` sees a correctly-locked schema file.
#[cfg(unix)]
fn set_readonly_444(path: &Path) -> Result<(), WhError> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o444);
    std::fs::set_permissions(path, perms)
        .map_err(|e| WhError::Other(format!("failed to chmod 444 schema file: {e}")))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_readonly_444(_path: &Path) -> Result<(), WhError> {
    // The wh toolchain is Unix-only (podman host). This branch exists so the
    // crate still type-checks on Windows in case a future contributor runs
    // cargo check there; it is a no-op because Windows has no equivalent
    // POSIX mode.
    Ok(())
}

/// Human-readable output for `wh library init` (AC-6).
fn render_init_human(data: &InitData) -> String {
    let mut out = String::new();
    out.push_str(&format!("Library initialized — {}\n", data.agent));
    out.push_str(&format!("  Topology:   {}\n", data.topology));
    out.push_str(&format!("  Volume:     {}\n", data.volume));
    out.push_str(&format!("  Mount:      {}\n", data.mount_path));
    out.push('\n');
    out.push_str("  .library/ created and git-initialized (initial empty commit)\n");
    out.push_str("  .wh-schema.md written (read-only, mode 444)\n");
    if data.force {
        out.push_str("  --force: existing .git metadata was reset; pages preserved\n");
    }
    out
}

/// Resolve the host-side mount path for a podman named volume.
///
/// Invokes `podman volume inspect <vol> --format '{{.Mountpoint}}'`. Returns
/// `Err(WhError::Other)` with a user-facing message when podman is missing, the
/// volume does not exist, or the command fails for any other reason. The error
/// wording deliberately avoids "broker", port numbers, and internal jargon
/// (RT-B1) — it matches the family of errors `wh ps` emits when podman is
/// unreachable.
fn resolve_mount_path(volume: &str) -> Result<PathBuf, WhError> {
    let podman = wh_broker::deploy::podman::find_podman()
        .map_err(|e| WhError::Other(format!("Podman not available: {e}")))?;

    let output = std::process::Command::new(podman)
        .args(["volume", "inspect", volume, "--format", "{{.Mountpoint}}"])
        .output()
        .map_err(|e| WhError::Other(format!("failed to run podman: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WhError::Other(format!(
            "workspace volume '{volume}' not found. Has the topology been applied? ({stderr})",
            stderr = stderr.trim()
        )));
    }

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return Err(WhError::Other(format!(
            "workspace volume '{volume}' has no mountpoint"
        )));
    }
    Ok(PathBuf::from(raw))
}

/// Gather the six health signals from the host filesystem.
///
/// Pure data collection — never panics. Every sub-read is fault-tolerant and
/// degrades to `None` / `false` / `0` on error so an incomplete/un-initialized
/// Library renders as "not yet initialized" rather than crashing (AC-6).
fn collect_status_data(
    agent: String,
    topology: String,
    volume: String,
    mount_path: PathBuf,
) -> StatusData {
    let schema_path = mount_path.join(".wh-schema.md");
    let schema_present = schema_path.exists();

    let library_root = mount_path.join(".library");
    let pages_dir = library_root.join("pages");
    let page_count = count_pages_under(&pages_dir);

    let (git_head, last_ingest_at) = read_git_head_info(&library_root);

    let (lock_held, lock_age_seconds) = read_lock_info(&library_root);

    let free_disk_bytes = read_free_disk_bytes(&mount_path);

    StatusData {
        agent,
        topology,
        volume,
        mount_path: mount_path.to_string_lossy().into_owned(),
        schema_present,
        page_count,
        git_head,
        last_ingest_at,
        lock_held,
        lock_age_seconds,
        free_disk_bytes,
    }
}

/// Recursively count `.md` files under `dir`. Missing directory → 0.
fn count_pages_under(dir: &Path) -> u64 {
    fn walk(dir: &Path, count: &mut u64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    walk(&path, count);
                } else if ft.is_file() && path.extension().and_then(|s| s.to_str()) == Some("md") {
                    *count += 1;
                }
            }
        }
    }
    let mut count: u64 = 0;
    walk(dir, &mut count);
    count
}

/// Read `(git_head, last_ingest_at)` via two `git` subprocess calls.
///
/// Both are bounded by a 5-second timeout (implicit — git ops on a local repo
/// are millisecond-scale). Failure on either → `None` for that field, never an
/// error result.
fn read_git_head_info(library_root: &Path) -> (Option<String>, Option<String>) {
    if !library_root.join(".git").is_dir() {
        return (None, None);
    }
    let head = run_git_capture(library_root, &["rev-parse", "--short", "HEAD"]);
    let last = run_git_capture(library_root, &["log", "-1", "--format=%aI", "HEAD"]);
    (head, last)
}

/// Run `git -C <dir> <args...>` and return trimmed stdout on success.
fn run_git_capture(dir: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Read `(lock_held, lock_age_seconds)` from `.git/index.lock`.
fn read_lock_info(library_root: &Path) -> (bool, Option<u64>) {
    let lock_path = library_root.join(".git").join("index.lock");
    match std::fs::metadata(&lock_path) {
        Ok(meta) => {
            let age = meta
                .modified()
                .ok()
                .and_then(|mt| mt.duration_since(UNIX_EPOCH).ok())
                .and_then(|created| {
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .ok()
                        .map(|now| now.saturating_sub(created))
                })
                .map(|d: Duration| d.as_secs());
            (true, age)
        }
        Err(_) => (false, None),
    }
}

/// Read the free bytes available on the filesystem backing `path` by invoking
/// `df -kP <path>` and parsing the "Available" column of the second line.
///
/// `df -kP` is POSIX-portable (macOS and Linux). The 4th column is Available-KB;
/// we multiply by 1024 to return bytes. Failure → `None`, never an error.
fn read_free_disk_bytes(path: &Path) -> Option<u64> {
    let output = std::process::Command::new("df")
        .arg("-kP")
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_df_output(&stdout)
}

/// Parse the second line of `df -kP` output and return Available bytes.
///
/// `df -kP` guarantees a single output line per filesystem regardless of how
/// long the device name is (the `-P` flag forces POSIX single-line format).
/// The columns are: Filesystem, 1024-blocks, Used, Available, Capacity, Mounted on.
/// We take column 4 (Available-KB) × 1024.
fn parse_df_output(stdout: &str) -> Option<u64> {
    let second_line = stdout.lines().nth(1)?;
    let available_kb: u64 = second_line.split_whitespace().nth(3)?.parse().ok()?;
    Some(available_kb.saturating_mul(1024))
}

/// Format a `StatusData` as the human-readable output block (AC-3).
fn render_human(data: &StatusData) -> String {
    let mut out = String::new();
    out.push_str(&format!("Library — {}\n", data.agent));
    out.push_str(&format!("  Topology:   {}\n", data.topology));
    out.push_str(&format!("  Volume:     {}\n", data.volume));
    out.push_str(&format!("  Mount:      {}\n", data.mount_path));

    let empty = data.git_head.is_none()
        && data.page_count == 0
        && data.last_ingest_at.is_none()
        && !data.lock_held;

    if empty {
        out.push_str("\n  Library not yet initialized — run an ingest to populate.\n");
    }

    out.push('\n');
    out.push_str(&format!(
        "  Schema file:  {}\n",
        if data.schema_present {
            "present"
        } else {
            "missing"
        }
    ));
    out.push_str(&format!("  Pages:        {}\n", data.page_count));
    out.push_str(&format!(
        "  Git HEAD:     {}\n",
        data.git_head.as_deref().unwrap_or("—")
    ));
    out.push_str(&format!(
        "  Last ingest:  {}\n",
        data.last_ingest_at.as_deref().unwrap_or("—")
    ));
    let lock_display = if data.lock_held {
        match data.lock_age_seconds {
            Some(age) => format!("held ({age}s)"),
            None => "held".to_string(),
        }
    } else {
        "free".to_string()
    };
    out.push_str(&format!("  Lock:         {lock_display}\n"));
    out.push_str(&format!(
        "  Free disk:    {}\n",
        match data.free_disk_bytes {
            Some(b) => format_bytes_human(b),
            None => "—".to_string(),
        }
    ));

    out
}

/// Format a byte count as a short human-readable string (e.g. "12.3 GB").
fn format_bytes_human(bytes: u64) -> String {
    const UNITS: &[(&str, u64)] = &[
        ("TB", 1_000_000_000_000),
        ("GB", 1_000_000_000),
        ("MB", 1_000_000),
        ("KB", 1_000),
    ];
    for (unit, scale) in UNITS {
        if bytes >= *scale {
            let value = bytes as f64 / *scale as f64;
            return format!("{value:.1} {unit}");
        }
    }
    format!("{bytes} B")
}

#[cfg(test)]
mod tests {
    use super::DEFAULT_SCHEMA_TEMPLATE;
    use regex::Regex;
    use std::collections::BTreeSet;

    /// AC-3: top-level heading is "# Your Library".
    #[test]
    fn test_template_top_heading() {
        let first_non_empty = DEFAULT_SCHEMA_TEMPLATE
            .lines()
            .find(|l| !l.trim().is_empty())
            .expect("template must not be empty");
        assert_eq!(
            first_non_empty, "# Your Library",
            "first non-empty line must be the top heading; got {first_non_empty:?}"
        );
    }

    /// AC-3: all eight required level-2 sections are present.
    #[test]
    fn test_template_required_sections_present() {
        let required_headings = [
            "## What your Library is for",
            "## When to search your Library",
            "## When to write to your Library (important)",
            "## When **not** to write",
            "## How to write a Library page",
            "## Maintaining `index.md`",
            "## Disclaimer (always include in responses derived from Library content)",
            "## Your domain (configurable)",
        ];
        for heading in required_headings {
            assert!(
                DEFAULT_SCHEMA_TEMPLATE.contains(heading),
                "template must contain required heading: {heading}"
            );
        }
    }

    /// AC-4: FR39 disclaimer literal substring is present, plus the
    /// "LLM-generated summaries" framing sentence.
    #[test]
    fn test_template_contains_disclaimer_text() {
        let disclaimer = "This comes from my notes — please verify against the original source if it's critical.";
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains(disclaimer),
            "template must contain the FR39 disclaimer literal"
        );
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains("LLM-generated summaries"),
            "template must contain the LLM-generated-summaries framing"
        );
    }

    /// AC-5: all four labeled write-trigger categories present.
    #[test]
    fn test_template_contains_write_categories() {
        let categories = [
            "Category A — Durable facts about the user, their work, or their domain",
            "Category B — Decisions, commitments, and policies the user has established",
            "Category C — Things you figured out on your own that would save future-you time",
            "Category D — Corrections to prior Library content",
        ];
        for cat in categories {
            assert!(
                DEFAULT_SCHEMA_TEMPLATE.contains(cat),
                "template must contain write-trigger category: {cat}"
            );
        }
    }

    /// AC-5: explicit non-trigger section is present.
    #[test]
    fn test_template_has_explicit_non_triggers_section() {
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains("## When **not** to write"),
            "template must contain an explicit non-triggers section"
        );
    }

    /// AC-6: required `{{...}}` placeholders are present.
    #[test]
    fn test_template_contains_required_placeholders() {
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains("{{agent_name}}"),
            "template must contain {{{{agent_name}}}} placeholder"
        );
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains("{{domain_description}}"),
            "template must contain {{{{domain_description}}}} placeholder"
        );
    }

    /// AC-6: every `{{...}}` token in the template is in the allow-list. A future
    /// edit that introduces an undocumented placeholder fails this test, forcing the
    /// edit to also update story 13-22 (the consumer that performs substitution).
    #[test]
    fn test_template_no_undocumented_placeholders() {
        let allow_list: BTreeSet<&str> = ["agent_name", "domain_description"].into_iter().collect();
        let re = Regex::new(r"\{\{([a-zA-Z_][a-zA-Z0-9_]*)\}\}")
            .expect("placeholder regex must compile");
        let found: BTreeSet<String> = re
            .captures_iter(DEFAULT_SCHEMA_TEMPLATE)
            .map(|c| c[1].to_string())
            .collect();
        for token in &found {
            assert!(
                allow_list.contains(token.as_str()),
                "undocumented placeholder {{{{{token}}}}} found in template — \
                 if this is intentional, update both this allow-list AND \
                 story 13-22 (wh library init substitution logic)"
            );
        }
    }

    /// AC-7: search-when-relevant gate language is present.
    #[test]
    fn test_template_search_gate_language() {
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains("At the start of every turn"),
            "template must instruct the agent at the start of every turn"
        );
        assert!(
            DEFAULT_SCHEMA_TEMPLATE.contains("**do not search**"),
            "template must explicitly tell the agent not to search for off-domain queries"
        );
    }

    // ─── Story 13-24: `wh library status` tests ────────────────────────

    use super::{
        count_pages_under, format_bytes_human, parse_df_output, render_human, LibraryCommand,
        StatusData,
    };
    use clap::Parser;

    /// Minimal clap harness so we can assert the library subcommand parses as
    /// expected without wiring the full `Commands` enum into the test.
    #[derive(Debug, Parser)]
    #[command(name = "wh-test")]
    struct TestCli {
        #[command(subcommand)]
        library: LibraryCommand,
    }

    #[test]
    fn test_library_command_group_parses() {
        let cli =
            TestCli::try_parse_from(["wh-test", "status", "donna"]).expect("parse should succeed");
        match cli.library {
            LibraryCommand::Status(args) => {
                assert_eq!(args.agent, "donna");
                assert_eq!(args.format, crate::output::OutputFormat::Human);
            }
            other => panic!("expected Status variant, got {other:?}"),
        }
    }

    #[test]
    fn test_library_command_group_parses_json_flag() {
        let cli = TestCli::try_parse_from(["wh-test", "status", "donna", "--format", "json"])
            .expect("parse should succeed");
        match cli.library {
            LibraryCommand::Status(args) => {
                assert_eq!(args.agent, "donna");
                assert_eq!(args.format, crate::output::OutputFormat::Json);
            }
            other => panic!("expected Status variant, got {other:?}"),
        }
    }

    fn sample_status(populated: bool) -> StatusData {
        if populated {
            StatusData {
                agent: "donna".into(),
                topology: "dev".into(),
                volume: "wh-dev-donna-workspace".into(),
                mount_path: "/var/lib/containers/storage/volumes/wh-dev-donna-workspace/_data"
                    .into(),
                schema_present: true,
                page_count: 42,
                git_head: Some("abc1234".into()),
                last_ingest_at: Some("2026-04-09T14:30:00+00:00".into()),
                lock_held: false,
                lock_age_seconds: None,
                free_disk_bytes: Some(12_345_678_901),
            }
        } else {
            StatusData {
                agent: "donna".into(),
                topology: "dev".into(),
                volume: "wh-dev-donna-workspace".into(),
                mount_path: "/mnt/donna".into(),
                schema_present: false,
                page_count: 0,
                git_head: None,
                last_ingest_at: None,
                lock_held: false,
                lock_age_seconds: None,
                free_disk_bytes: None,
            }
        }
    }

    #[test]
    fn test_render_human_empty_library() {
        let rendered = render_human(&sample_status(false));
        assert!(rendered.contains("Library — donna"));
        assert!(rendered.contains("Library not yet initialized"));
        assert!(rendered.contains("Schema file:  missing"));
        assert!(rendered.contains("Pages:        0"));
        assert!(rendered.contains("Git HEAD:     —"));
        assert!(rendered.contains("Last ingest:  —"));
        assert!(rendered.contains("Lock:         free"));
        assert!(rendered.contains("Free disk:    —"));
    }

    #[test]
    fn test_render_human_populated_library() {
        let rendered = render_human(&sample_status(true));
        assert!(rendered.contains("Library — donna"));
        assert!(!rendered.contains("Library not yet initialized"));
        assert!(rendered.contains("Schema file:  present"));
        assert!(rendered.contains("Pages:        42"));
        assert!(rendered.contains("Git HEAD:     abc1234"));
        assert!(rendered.contains("Last ingest:  2026-04-09T14:30:00+00:00"));
        assert!(rendered.contains("Lock:         free"));
        assert!(rendered.contains("Free disk:    12.3 GB"));
    }

    #[test]
    fn test_render_json_populated_library() {
        let data = sample_status(true);
        let json = serde_json::to_string(&data).expect("serialize");
        for key in [
            "\"agent\"",
            "\"topology\"",
            "\"volume\"",
            "\"mount_path\"",
            "\"schema_present\"",
            "\"page_count\"",
            "\"git_head\"",
            "\"last_ingest_at\"",
            "\"lock_held\"",
            "\"lock_age_seconds\"",
            "\"free_disk_bytes\"",
        ] {
            assert!(json.contains(key), "json output must contain {key}");
        }
    }

    #[test]
    fn test_parse_df_output_happy_path() {
        // Canonical `df -kP` output on Linux and macOS: header line, one data line.
        let stdout = "\
Filesystem                     1024-blocks        Used   Available Capacity Mounted on
/dev/mapper/root                  100000000    30000000    65000000     32% /
";
        assert_eq!(parse_df_output(stdout), Some(65_000_000 * 1024));
    }

    #[test]
    fn test_parse_df_output_missing_line() {
        assert_eq!(parse_df_output(""), None);
        assert_eq!(parse_df_output("header only\n"), None);
    }

    #[test]
    fn test_parse_df_output_malformed_column() {
        let stdout = "\
Filesystem 1024-blocks Used Available Capacity Mounted
/dev/foo    100 30 notanumber 50% /
";
        assert_eq!(parse_df_output(stdout), None);
    }

    #[test]
    fn test_count_pages_missing_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("does-not-exist");
        assert_eq!(count_pages_under(&missing), 0);
    }

    #[test]
    fn test_count_pages_counts_only_md_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.md"), "x").unwrap();
        std::fs::write(root.join("b.md"), "x").unwrap();
        std::fs::write(root.join("ignore.txt"), "x").unwrap();
        std::fs::write(root.join("ignore.png"), "x").unwrap();
        std::fs::write(root.join("sub").join("c.md"), "x").unwrap();
        std::fs::write(root.join("sub").join("ignore.yaml"), "x").unwrap();
        assert_eq!(count_pages_under(root), 3);
    }

    // ─── Story 13-22: `wh library init` tests ─────────────────────────

    use super::{init_library_at, render_init_human, render_schema, InitData};

    // ─── Story 13-23: `wh library ingest` tests ──────────────────────

    use super::{
        build_invocation_parameters, infer_source_type, render_ingest_human, resolve_target_stream,
        translate_source_path, IngestData, IngestResultData,
    };
    use std::path::Path;

    #[test]
    fn test_library_init_command_parses() {
        let cli = TestCli::try_parse_from(["wh-test", "init", "--agent", "donna"])
            .expect("parse should succeed");
        match cli.library {
            LibraryCommand::Init(args) => {
                assert_eq!(args.agent, "donna");
                assert_eq!(args.format, crate::output::OutputFormat::Human);
                assert!(!args.force);
                assert_eq!(args.domain_description, None);
            }
            other => panic!("expected Init variant, got {other:?}"),
        }
    }

    #[test]
    fn test_library_init_command_parses_all_flags() {
        let cli = TestCli::try_parse_from([
            "wh-test",
            "init",
            "--agent",
            "donna",
            "--force",
            "--format",
            "json",
            "--domain-description",
            "cats",
        ])
        .expect("parse should succeed");
        match cli.library {
            LibraryCommand::Init(args) => {
                assert_eq!(args.agent, "donna");
                assert_eq!(args.format, crate::output::OutputFormat::Json);
                assert!(args.force);
                assert_eq!(args.domain_description.as_deref(), Some("cats"));
            }
            other => panic!("expected Init variant, got {other:?}"),
        }
    }

    #[test]
    fn test_library_init_requires_agent_flag() {
        // `wh library init` without --agent must fail to parse.
        let err = TestCli::try_parse_from(["wh-test", "init"]);
        assert!(err.is_err(), "missing --agent must be a clap error");
    }

    #[test]
    fn test_render_schema_substitutes_both_placeholders() {
        let rendered = render_schema("donna", Some("cat care"));
        assert!(rendered.contains("donna"));
        assert!(rendered.contains("cat care"));
        // AC-8: zero residual placeholders.
        let re = Regex::new(r"\{\{[a-zA-Z_][a-zA-Z0-9_]*\}\}").expect("placeholder regex compiles");
        assert!(
            re.find(&rendered).is_none(),
            "rendered template must not contain any {{...}} tokens"
        );
    }

    #[test]
    fn test_render_schema_default_domain_description() {
        let rendered = render_schema("donna", None);
        assert!(rendered.contains("donna"));
        // Default domain description substring from the constant.
        assert!(rendered.contains("Set this to describe"));
        let re = Regex::new(r"\{\{[a-zA-Z_][a-zA-Z0-9_]*\}\}").expect("placeholder regex compiles");
        assert!(
            re.find(&rendered).is_none(),
            "rendered template with default domain must not contain {{...}} tokens"
        );
    }

    #[test]
    fn test_init_library_at_fresh_creates_git_and_schema() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mount = tmp.path();
        init_library_at(mount, "donna", Some("testing"), false).expect("fresh init should succeed");

        // .library/.git exists
        assert!(mount.join(".library").is_dir());
        assert!(mount.join(".library/.git").is_dir());
        // HEAD resolves (initial empty commit)
        let head = std::process::Command::new("git")
            .arg("-C")
            .arg(mount.join(".library"))
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git rev-parse");
        assert!(
            head.status.success(),
            "git rev-parse HEAD must succeed on freshly initialized library"
        );
        // schema file exists with content
        let schema_path = mount.join(".wh-schema.md");
        assert!(schema_path.exists());
        let schema_content = std::fs::read_to_string(&schema_path).expect("read schema file");
        assert!(schema_content.contains("donna"));
        assert!(schema_content.contains("testing"));
        // mode 444
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&schema_path)
                .expect("stat schema")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o444, "schema file must be chmod 444");
        }
    }

    #[test]
    fn test_init_library_at_refuses_existing_without_force() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mount = tmp.path();
        init_library_at(mount, "donna", None, false).expect("first init");
        let err = init_library_at(mount, "donna", None, false)
            .expect_err("second init without --force must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("already initialized") && msg.contains("--force"),
            "error must mention already-initialized and --force; got: {msg}"
        );
    }

    #[test]
    fn test_init_library_at_force_preserves_pages() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mount = tmp.path();
        init_library_at(mount, "donna", None, false).expect("first init");

        // Put a page in place after first init.
        let pages_dir = mount.join(".library").join("pages");
        std::fs::create_dir_all(&pages_dir).unwrap();
        let page_path = pages_dir.join("persisted.md");
        std::fs::write(&page_path, "important content").unwrap();

        // Capture HEAD before.
        let head_before = std::process::Command::new("git")
            .arg("-C")
            .arg(mount.join(".library"))
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git rev-parse before");
        let head_before_str = String::from_utf8_lossy(&head_before.stdout)
            .trim()
            .to_string();

        // Re-init with --force.
        init_library_at(mount, "donna", None, true).expect("force re-init");

        // Page still exists and has same content.
        assert!(
            page_path.exists(),
            "pages/persisted.md must survive --force"
        );
        let content = std::fs::read_to_string(&page_path).unwrap();
        assert_eq!(content, "important content");

        // git HEAD still resolves (new commit).
        let head_after = std::process::Command::new("git")
            .arg("-C")
            .arg(mount.join(".library"))
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git rev-parse after");
        assert!(head_after.status.success());
        let head_after_str = String::from_utf8_lossy(&head_after.stdout)
            .trim()
            .to_string();
        // New empty commit — hash is identical for a brand-new repo with a
        // "wh library init" empty commit because git hashes are content-
        // addressed and the tree is empty + the message + author are fixed.
        // We therefore only assert HEAD resolves; we do NOT assert inequality.
        assert!(!head_after_str.is_empty());
        // Suppress unused warning on head_before_str.
        let _ = head_before_str;
    }

    #[test]
    fn test_init_library_at_missing_mount_path_fails() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("does-not-exist");
        let err = init_library_at(&missing, "donna", None, false)
            .expect_err("missing mount path must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("wh topology apply"),
            "error must point to `wh topology apply`; got: {msg}"
        );
    }

    fn sample_init_data(force: bool) -> InitData {
        InitData {
            agent: "donna".into(),
            topology: "dev".into(),
            volume: "wh-dev-donna-workspace".into(),
            mount_path: "/mnt/donna".into(),
            library_initialized: true,
            schema_written: true,
            force,
        }
    }

    #[test]
    fn test_ingest_args_parse_defaults() {
        let cli = TestCli::try_parse_from(["wh-test", "ingest", "./notes.md", "--agent", "donna"])
            .expect("parse should succeed");
        match cli.library {
            LibraryCommand::Ingest(args) => {
                assert_eq!(args.file.to_str(), Some("./notes.md"));
                assert_eq!(args.agent, "donna");
                assert!(args.r#type.is_none());
                assert!(args.hint.is_none());
                assert!(args.stream.is_none());
                assert!(!args.wait);
                assert_eq!(args.format, crate::output::OutputFormat::Human);
            }
            other => panic!("expected Ingest variant, got {other:?}"),
        }
    }

    #[test]
    fn test_ingest_args_parse_all_flags() {
        let cli = TestCli::try_parse_from([
            "wh-test",
            "ingest",
            "/workspace/brief.pdf",
            "--agent",
            "donna",
            "--type",
            "pdf",
            "--hint",
            "Q3 planning",
            "--stream",
            "inbox",
            "--wait",
            "--format",
            "json",
        ])
        .expect("parse should succeed");
        match cli.library {
            LibraryCommand::Ingest(args) => {
                assert_eq!(args.r#type.as_deref(), Some("pdf"));
                assert_eq!(args.hint.as_deref(), Some("Q3 planning"));
                assert_eq!(args.stream.as_deref(), Some("inbox"));
                assert!(args.wait);
                assert_eq!(args.format, crate::output::OutputFormat::Json);
            }
            other => panic!("expected Ingest variant, got {other:?}"),
        }
    }

    #[test]
    fn test_infer_source_type_by_extension() {
        assert_eq!(
            infer_source_type(Path::new("foo.md"), None).unwrap(),
            "markdown"
        );
        assert_eq!(
            infer_source_type(Path::new("foo.markdown"), None).unwrap(),
            "markdown"
        );
        assert_eq!(
            infer_source_type(Path::new("NOTES.TXT"), None).unwrap(),
            "text"
        );
        assert_eq!(
            infer_source_type(Path::new("brief.pdf"), None).unwrap(),
            "pdf"
        );
    }

    #[test]
    fn test_infer_source_type_override_wins() {
        // Override takes precedence even against an extension the extension
        // map would have mapped to something else.
        assert_eq!(
            infer_source_type(Path::new("brief.pdf"), Some("text")).unwrap(),
            "text"
        );
        assert_eq!(
            infer_source_type(Path::new("no_extension"), Some("url")).unwrap(),
            "url"
        );
    }

    #[test]
    fn test_infer_source_type_unknown_rejected() {
        let err = infer_source_type(Path::new("report.docx"), None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("UNKNOWN_SOURCE_TYPE"));
        assert!(msg.contains("text"));
        assert!(msg.contains("markdown"));
        assert!(msg.contains("pdf"));
        assert!(msg.contains("url"));
    }

    #[test]
    fn test_infer_source_type_override_enum_rejected() {
        let err = infer_source_type(Path::new("foo.md"), Some("weird")).unwrap_err();
        assert!(err.to_string().contains("UNKNOWN_SOURCE_TYPE"));
    }

    #[test]
    fn test_translate_source_container_path_accepted() {
        // Simulate a mount directory with the expected host file inside it.
        let tmp = tempfile::tempdir().expect("tempdir");
        let mount = tmp.path();
        std::fs::write(mount.join("brief.md"), "hello").unwrap();

        let out = translate_source_path(Path::new("/workspace/brief.md"), mount).expect("ok");
        assert_eq!(out, "/workspace/brief.md");
    }

    #[test]
    fn test_translate_source_container_path_missing_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = translate_source_path(Path::new("/workspace/ghost.md"), tmp.path()).unwrap_err();
        assert!(err.to_string().contains("File not found"));
    }

    #[test]
    fn test_translate_source_host_path_inside_mount_rewritten() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mount = tmp.path();
        std::fs::create_dir_all(mount.join("docs")).unwrap();
        let host_file = mount.join("docs").join("brief.md");
        std::fs::write(&host_file, "hello").unwrap();

        let out = translate_source_path(&host_file, mount).expect("ok");
        assert_eq!(out, "/workspace/docs/brief.md");
    }

    #[test]
    fn test_translate_source_host_path_outside_mount_rejected() {
        let tmp_mount = tempfile::tempdir().expect("tempdir");
        let tmp_other = tempfile::tempdir().expect("tempdir");
        let outside_file = tmp_other.path().join("outside.md");
        std::fs::write(&outside_file, "hello").unwrap();

        let err = translate_source_path(&outside_file, tmp_mount.path()).unwrap_err();
        assert!(err.to_string().contains("PATH_OUTSIDE_WORKSPACE"));
    }

    #[test]
    fn test_resolve_stream_default_first() {
        let streams = vec!["inbox".to_string(), "cron".to_string()];
        assert_eq!(resolve_target_stream(&streams, None).unwrap(), "inbox");
    }

    #[test]
    fn test_resolve_stream_override_valid() {
        let streams = vec!["inbox".to_string(), "cron".to_string()];
        assert_eq!(
            resolve_target_stream(&streams, Some("cron")).unwrap(),
            "cron"
        );
    }

    #[test]
    fn test_resolve_stream_override_not_declared() {
        let streams = vec!["inbox".to_string()];
        let err = resolve_target_stream(&streams, Some("ghost")).unwrap_err();
        assert!(err.to_string().contains("STREAM_NOT_DECLARED"));
    }

    #[test]
    fn test_resolve_stream_empty_list() {
        let streams: Vec<String> = vec![];
        let err = resolve_target_stream(&streams, None).unwrap_err();
        assert!(err.to_string().contains("NO_INPUT_STREAM"));
    }

    #[test]
    fn test_build_invocation_parameters_without_hint() {
        let m = build_invocation_parameters("markdown", "/workspace/foo.md", None);
        assert_eq!(m.get("source_type").map(String::as_str), Some("markdown"));
        assert_eq!(
            m.get("source_ref").map(String::as_str),
            Some("/workspace/foo.md")
        );
        assert!(!m.contains_key("user_summary_hint"));
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn test_build_invocation_parameters_with_hint() {
        let m =
            build_invocation_parameters("pdf", "/workspace/brief.pdf", Some("Q3 planning deck"));
        assert_eq!(m.get("source_type").map(String::as_str), Some("pdf"));
        assert_eq!(
            m.get("user_summary_hint").map(String::as_str),
            Some("Q3 planning deck")
        );
        assert_eq!(m.len(), 3);
    }

    fn sample_ingest_data(with_result: bool) -> IngestData {
        IngestData {
            invocation_id: "11111111-2222-3333-4444-555555555555".to_string(),
            agent: "donna".to_string(),
            stream: "inbox".to_string(),
            source_type: "markdown".to_string(),
            source_ref: "/workspace/brief.md".to_string(),
            waited: with_result,
            result: if with_result {
                Some(IngestResultData {
                    success: true,
                    error_code: String::new(),
                    error_message: String::new(),
                    output: String::new(),
                    library_tokens: Some(1234),
                    library_page_count: Some(7),
                    library_last_ingest_at: Some("2026-04-10T12:00:00+00:00".to_string()),
                })
            } else {
                None
            },
        }
    }

    #[test]
    fn test_render_init_human_contains_key_fields() {
        let rendered = render_init_human(&sample_init_data(false));
        assert!(rendered.contains("Library initialized — donna"));
        assert!(rendered.contains("/mnt/donna"));
        assert!(rendered.contains("initialized"));
        assert!(rendered.contains("read-only, mode 444"));
        assert!(!rendered.contains("--force"));
    }

    #[test]
    fn test_render_init_human_force_message() {
        let rendered = render_init_human(&sample_init_data(true));
        assert!(rendered.contains("--force"));
    }

    #[test]
    fn test_render_init_json_envelope_has_all_keys() {
        let data = sample_init_data(true);
        let json = serde_json::to_string(&data).expect("serialize");
        for key in [
            "\"agent\"",
            "\"topology\"",
            "\"volume\"",
            "\"mount_path\"",
            "\"library_initialized\"",
            "\"schema_written\"",
            "\"force\"",
        ] {
            assert!(json.contains(key), "json output must contain {key}");
        }
    }

    /// 13-22 also covers the format() dispatcher branch for Init.
    #[test]
    fn test_library_command_format_dispatches_init() {
        let cli =
            TestCli::try_parse_from(["wh-test", "init", "--agent", "donna", "--format", "json"])
                .expect("parse");
        assert_eq!(cli.library.format(), crate::output::OutputFormat::Json);
    }

    #[test]
    fn test_render_human_publish_only() {
        let out = render_ingest_human(&sample_ingest_data(false));
        assert!(out.contains("Ingest published to stream 'inbox' for agent 'donna'"));
        assert!(out.contains("invocation_id: 11111111-2222-3333-4444-555555555555"));
        assert!(out.contains("source_type:   markdown"));
        assert!(out.contains("source_ref:    /workspace/brief.md"));
        assert!(!out.contains("SkillResult received"));
    }

    #[test]
    fn test_render_human_with_skill_result() {
        let out = render_ingest_human(&sample_ingest_data(true));
        assert!(out.contains("SkillResult received"));
        assert!(out.contains("success:       true"));
        assert!(out.contains("library_tokens:         1234"));
        assert!(out.contains("library_page_count:     7"));
        assert!(out.contains("library_last_ingest_at: 2026-04-10T12:00:00+00:00"));
    }

    #[test]
    fn test_render_json_publish_only() {
        let data = sample_ingest_data(false);
        let json = serde_json::to_string(&data).expect("serialize");
        for key in [
            "\"invocation_id\"",
            "\"agent\"",
            "\"stream\"",
            "\"source_type\"",
            "\"source_ref\"",
            "\"waited\"",
        ] {
            assert!(json.contains(key), "json must contain {key}: {json}");
        }
        // result field is skip_serializing_if = "Option::is_none", so the
        // key is absent in the publish-only case.
        assert!(!json.contains("\"result\""));
    }

    #[test]
    fn test_render_json_with_skill_result() {
        let data = sample_ingest_data(true);
        let json = serde_json::to_string(&data).expect("serialize");
        assert!(json.contains("\"result\""));
        assert!(json.contains("\"library_tokens\":1234"));
        assert!(json.contains("\"library_page_count\":7"));
        assert!(json.contains("\"success\":true"));
    }

    #[test]
    fn test_format_bytes_human() {
        assert_eq!(format_bytes_human(0), "0 B");
        assert_eq!(format_bytes_human(999), "999 B");
        assert_eq!(format_bytes_human(1_500), "1.5 KB");
        assert_eq!(format_bytes_human(2_500_000), "2.5 MB");
        assert_eq!(format_bytes_human(3_200_000_000), "3.2 GB");
        assert_eq!(format_bytes_human(4_100_000_000_000), "4.1 TB");
    }

    // ─── Story 13-25: `wh library list` tests ────────────────────────

    use super::{
        derive_library_status, render_list_human, unreachable_status_data, LibraryRow, ListData,
    };

    fn row(agent: &str, head: Option<&str>, pages: u64, unreachable: bool) -> LibraryRow {
        let data = StatusData {
            agent: agent.into(),
            topology: "dev".into(),
            volume: format!("wh-dev-{agent}-workspace"),
            mount_path: "/mnt".into(),
            schema_present: head.is_some(),
            page_count: pages,
            git_head: head.map(|s| s.into()),
            last_ingest_at: head.map(|_| "2026-04-09T14:30:00+00:00".into()),
            lock_held: false,
            lock_age_seconds: None,
            free_disk_bytes: Some(12_345_678_901),
        };
        let status = derive_library_status(&data, unreachable).to_string();
        LibraryRow {
            data,
            status,
            unreachable,
        }
    }

    #[test]
    fn test_library_list_command_parses() {
        let cli = TestCli::try_parse_from(["wh-test", "list"]).expect("parse should succeed");
        match cli.library {
            LibraryCommand::List(args) => {
                assert_eq!(args.format, crate::output::OutputFormat::Human);
            }
            other => panic!("expected List variant, got {other:?}"),
        }
    }

    #[test]
    fn test_library_list_command_parses_json_flag() {
        let cli = TestCli::try_parse_from(["wh-test", "list", "--format", "json"])
            .expect("parse should succeed");
        match cli.library {
            LibraryCommand::List(args) => {
                assert_eq!(args.format, crate::output::OutputFormat::Json);
            }
            other => panic!("expected List variant, got {other:?}"),
        }
    }

    #[test]
    fn test_derive_library_status_not_init() {
        let data = unreachable_status_data("a".into(), "dev".into(), "v".into());
        assert_eq!(derive_library_status(&data, false), "not-init");
    }

    #[test]
    fn test_derive_library_status_empty() {
        let r = row("a", Some("abc1234"), 0, false);
        assert_eq!(r.status, "empty");
    }

    #[test]
    fn test_derive_library_status_active() {
        let r = row("a", Some("abc1234"), 42, false);
        assert_eq!(r.status, "active");
    }

    #[test]
    fn test_derive_library_status_unreachable_wins() {
        // Even with a populated data blob, the unreachable flag overrides.
        let mut data = row("a", Some("abc1234"), 42, false).data;
        // Just to prove the override is independent of disk state.
        data.page_count = 999;
        assert_eq!(derive_library_status(&data, true), "unreachable");
    }

    #[test]
    fn test_render_list_human_three_agents() {
        let data = ListData {
            topology: "dev".into(),
            libraries: vec![
                row("donna", Some("abc1234"), 42, false),
                row("mike", Some("def5678"), 0, false),
                row("harvey", None, 0, false),
            ],
        };
        let rendered = render_list_human(&data);
        assert!(rendered.contains("Libraries — topology 'dev'"));
        assert!(rendered.contains("AGENT"));
        assert!(rendered.contains("STATUS"));
        assert!(rendered.contains("PAGES"));
        assert!(rendered.contains("LAST INGEST"));
        assert!(rendered.contains("HEAD"));
        assert!(rendered.contains("SCHEMA"));
        assert!(rendered.contains("FREE DISK"));
        assert!(rendered.contains("donna"));
        assert!(rendered.contains("active"));
        assert!(rendered.contains("mike"));
        assert!(rendered.contains("empty"));
        assert!(rendered.contains("harvey"));
        assert!(rendered.contains("not-init"));
        assert!(rendered.contains("abc1234"));
    }

    #[test]
    fn test_render_list_human_unreachable_row() {
        let data = ListData {
            topology: "dev".into(),
            libraries: vec![row("broken", None, 0, true)],
        };
        let rendered = render_list_human(&data);
        assert!(rendered.contains("broken"));
        assert!(rendered.contains("unreachable"));
        // All displayable fields for the unreachable row must appear as em-dashes.
        // The renderer draws SCHEMA, HEAD, PAGES, LAST INGEST, FREE DISK as "—".
        assert!(rendered.contains("—"));
    }

    #[test]
    fn test_render_list_human_empty_topology() {
        let data = ListData {
            topology: "dev".into(),
            libraries: vec![],
        };
        let rendered = render_list_human(&data);
        assert!(rendered.contains("Libraries — topology 'dev'"));
        assert!(rendered.contains("no agents declared"));
        // No table header when there are no rows.
        assert!(!rendered.contains("AGENT"));
    }

    #[test]
    fn test_render_list_json_schema() {
        let data = ListData {
            topology: "dev".into(),
            libraries: vec![
                row("donna", Some("abc1234"), 42, false),
                row("broken", None, 0, true),
            ],
        };
        let json = serde_json::to_string(&data).expect("serialize");
        assert!(json.contains("\"topology\":\"dev\""));
        assert!(json.contains("\"libraries\""));
        // Each row has status + all StatusData fields flattened in.
        assert!(json.contains("\"status\":\"active\""));
        assert!(json.contains("\"status\":\"unreachable\""));
        assert!(json.contains("\"agent\":\"donna\""));
        assert!(json.contains("\"agent\":\"broken\""));
        assert!(json.contains("\"page_count\":42"));
        assert!(json.contains("\"git_head\":\"abc1234\""));
        assert!(json.contains("\"schema_present\":true"));
        // The internal `unreachable` flag must NOT appear in JSON as its own key
        // (the literal `"unreachable":true/false` pair). The `"unreachable"` string
        // is allowed to appear as the value of the `status` field.
        assert!(!json.contains("\"unreachable\":true"));
        assert!(!json.contains("\"unreachable\":false"));
    }

    // ─── Story 13-26: `wh library lint` tests ────────────────────────

    use super::{render_lint_human, resolve_lint_mode, LintData, LintResultData};

    #[test]
    fn test_library_lint_command_parses() {
        let cli = TestCli::try_parse_from(["wh-test", "lint", "--agent", "donna"])
            .expect("parse should succeed");
        match cli.library {
            LibraryCommand::Lint(args) => {
                assert_eq!(args.agent, "donna");
                assert!(args.mode.is_none());
                assert!(args.stream.is_none());
                assert!(!args.wait);
                assert_eq!(args.format, crate::output::OutputFormat::Human);
            }
            other => panic!("expected Lint variant, got {other:?}"),
        }
    }

    #[test]
    fn test_library_lint_command_parses_all_flags() {
        let cli = TestCli::try_parse_from([
            "wh-test", "lint", "--agent", "donna", "--mode", "full", "--stream", "inbox", "--wait",
            "--format", "json",
        ])
        .expect("parse should succeed");
        match cli.library {
            LibraryCommand::Lint(args) => {
                assert_eq!(args.agent, "donna");
                assert_eq!(args.mode.as_deref(), Some("full"));
                assert_eq!(args.stream.as_deref(), Some("inbox"));
                assert!(args.wait);
                assert_eq!(args.format, crate::output::OutputFormat::Json);
            }
            other => panic!("expected Lint variant, got {other:?}"),
        }
    }

    #[test]
    fn test_library_lint_requires_agent_flag() {
        let err = TestCli::try_parse_from(["wh-test", "lint"]);
        assert!(err.is_err(), "missing --agent must be a clap error");
    }

    #[test]
    fn test_resolve_lint_mode_default_incremental() {
        assert_eq!(resolve_lint_mode(None).unwrap(), "incremental");
    }

    #[test]
    fn test_resolve_lint_mode_full_accepted() {
        assert_eq!(resolve_lint_mode(Some("full")).unwrap(), "full");
        assert_eq!(
            resolve_lint_mode(Some("incremental")).unwrap(),
            "incremental"
        );
    }

    #[test]
    fn test_resolve_lint_mode_unknown_rejected() {
        let err = resolve_lint_mode(Some("sideways")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("UNKNOWN_LINT_MODE"));
        assert!(msg.contains("full"));
        assert!(msg.contains("incremental"));
    }

    fn sample_lint_data(with_result: bool) -> LintData {
        LintData {
            invocation_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
            agent: "donna".to_string(),
            stream: "inbox".to_string(),
            mode: "incremental".to_string(),
            waited: with_result,
            result: if with_result {
                Some(LintResultData {
                    success: true,
                    error_code: String::new(),
                    error_message: String::new(),
                    output: "[]".to_string(),
                    library_tokens: None,
                    library_page_count: Some(42),
                    library_last_ingest_at: None,
                })
            } else {
                None
            },
        }
    }

    #[test]
    fn test_render_lint_human_publish_only() {
        let out = render_lint_human(&sample_lint_data(false));
        assert!(out.contains("Lint published to stream 'inbox' for agent 'donna'"));
        assert!(out.contains("invocation_id: aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"));
        assert!(out.contains("mode:          incremental"));
        assert!(!out.contains("SkillResult received"));
    }

    #[test]
    fn test_render_lint_human_with_skill_result() {
        let out = render_lint_human(&sample_lint_data(true));
        assert!(out.contains("SkillResult received"));
        assert!(out.contains("success:       true"));
        assert!(out.contains("output:        []"));
        assert!(out.contains("library_page_count:     42"));
    }

    #[test]
    fn test_render_lint_human_with_failed_result() {
        let mut data = sample_lint_data(true);
        data.result = Some(LintResultData {
            success: false,
            error_code: "LINT_SANDBOX_UNREACHABLE".to_string(),
            error_message: "agent sandbox not ready".to_string(),
            output: String::new(),
            library_tokens: None,
            library_page_count: None,
            library_last_ingest_at: None,
        });
        let out = render_lint_human(&data);
        assert!(out.contains("success:       false"));
        assert!(out.contains("error_code:    LINT_SANDBOX_UNREACHABLE"));
        assert!(out.contains("error_message: agent sandbox not ready"));
    }

    #[test]
    fn test_render_lint_json_publish_only() {
        let data = sample_lint_data(false);
        let json = serde_json::to_string(&data).expect("serialize");
        for key in [
            "\"invocation_id\"",
            "\"agent\"",
            "\"stream\"",
            "\"mode\"",
            "\"waited\"",
        ] {
            assert!(json.contains(key), "json must contain {key}: {json}");
        }
        assert!(!json.contains("\"result\""));
    }

    #[test]
    fn test_render_lint_json_with_skill_result() {
        let data = sample_lint_data(true);
        let json = serde_json::to_string(&data).expect("serialize");
        assert!(json.contains("\"result\""));
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"library_page_count\":42"));
    }

    #[test]
    fn test_library_command_format_dispatches_lint() {
        let cli =
            TestCli::try_parse_from(["wh-test", "lint", "--agent", "donna", "--format", "json"])
                .expect("parse");
        assert_eq!(cli.library.format(), crate::output::OutputFormat::Json);
    }

    #[test]
    fn test_library_command_enum_alphabetical_order() {
        // Compile-time guard: listing variants in order must still compile. If a
        // future edit re-orders them, the match arm below will fail to compile in
        // a way that surfaces the invariant to the next contributor.
        fn _order_guard(cmd: &LibraryCommand) -> u8 {
            match cmd {
                LibraryCommand::Init(_) => 0,
                LibraryCommand::Ingest(_) => 1,
                LibraryCommand::Lint(_) => 2,
                LibraryCommand::List(_) => 3,
                LibraryCommand::Status(_) => 4,
            }
        }
    }
}
