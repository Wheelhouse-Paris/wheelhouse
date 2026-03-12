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
