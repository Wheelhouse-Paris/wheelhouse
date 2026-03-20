//! `.wh` file data model.
//!
//! Serde structs for parsing and validating Wheelhouse topology files.
//! All YAML keys use `snake_case` except `apiVersion` which is the only camelCase field.

use serde::{Deserialize, Serialize};

/// Top-level `.wh` file structure.
///
/// ```yaml
/// apiVersion: wheelhouse.dev/v1
/// agents:
///   - name: researcher
///     image: my-org/researcher:latest
///     max_replicas: 3
///     streams: [main]
/// streams:
///   - name: main
///     provider: local
///     compaction_cron: "0 2 * * *"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhFile {
    /// Must be `"wheelhouse.dev/v1"` for MVP.
    #[serde(rename = "apiVersion")]
    pub api_version: Option<String>,

    /// Agent specifications. May be absent or empty for skeleton files.
    pub agents: Option<Vec<AgentSpec>>,

    /// Stream specifications. May be absent or empty for skeleton files.
    pub streams: Option<Vec<StreamSpec>>,

    /// Surface specifications. May be absent or empty.
    pub surfaces: Option<Vec<SurfaceSpec>>,

    /// Optional broker configuration (ADR-029).
    /// When absent, native process fallback is used (deprecated).
    pub broker: Option<BrokerCliSpec>,
}

/// Broker specification within a `.wh` file (ADR-029).
///
/// Declares that the broker should run as a container with the given image
/// and optional port mappings. When absent from the `.wh` file, the native
/// process fallback is used (deprecated — `wh deploy lint` emits a warning).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerCliSpec {
    /// Container image for the broker (e.g., `ghcr.io/wheelhouse-paris/wh-broker:latest`).
    pub image: Option<String>,

    /// Port mappings published on the host (e.g., `["127.0.0.1:5555:5555"]`).
    pub ports: Option<Vec<String>>,
}

/// Agent specification within a `.wh` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpec {
    /// Unique agent name.
    pub name: Option<String>,

    /// Container image reference (e.g., `my-org/researcher:latest`).
    pub image: Option<String>,

    /// Maximum replica count. REQUIRED — guardrail to prevent unconstrained scaling.
    pub max_replicas: Option<u32>,

    /// Streams this agent subscribes to. References must match declared stream names.
    pub streams: Option<Vec<String>>,
}

/// Stream specification within a `.wh` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamSpec {
    /// Unique stream name.
    pub name: Option<String>,

    /// Stream provider. Only `"local"` supported in MVP (FR12). Defaults to `"local"` if absent.
    pub provider: Option<String>,

    /// Compaction cron expression. Warning if absent (FM-06).
    pub compaction_cron: Option<String>,
}

/// Surface specification within a `.wh` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurfaceSpec {
    /// Unique surface name.
    pub name: Option<String>,

    /// Surface type: "telegram" or "cli".
    pub kind: Option<String>,

    /// Stream name this surface connects to (single-stream mode).
    /// Mutually exclusive with `chats`.
    pub stream: Option<String>,

    /// Optional environment variables for the surface container.
    pub env: Option<std::collections::BTreeMap<String, String>>,

    /// Multi-chat configuration for Telegram surfaces.
    /// Each entry maps a chat (DM or supergroup) to one or more streams.
    /// Mutually exclusive with `stream`.
    pub chats: Option<Vec<TelegramChatSpec>>,
}

/// A Telegram chat specification within a surface's `chats` block.
///
/// Represents either:
/// - A DM chat (`@username`) with a single `stream`
/// - A supergroup (by display name) with multiple `threads`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramChatSpec {
    /// Chat identifier: `@username` for DMs, or group display name for supergroups.
    pub id: Option<String>,

    /// Stream name for DM chats (mutually exclusive with `threads` within a chat entry).
    pub stream: Option<String>,

    /// Thread (topic) list for supergroup chats.
    pub threads: Option<Vec<TelegramThreadSpec>>,
}

/// A Telegram topic/thread specification within a supergroup chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramThreadSpec {
    /// Topic name (human-readable, resolved to thread_id at runtime).
    pub id: Option<String>,

    /// Stream name this topic is bridged to.
    pub stream: Option<String>,
}
