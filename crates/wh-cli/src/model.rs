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

    /// Stream name this surface connects to.
    pub stream: Option<String>,

    /// Optional environment variables for the surface container.
    pub env: Option<std::collections::BTreeMap<String, String>>,
}
