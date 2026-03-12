//! Deploy pipeline: lint -> plan -> commit -> apply
//!
//! Implements the typestate pattern (5W-03) where the compiler enforces
//! the correct ordering of deploy operations:
//! `LintedFile -> PlanOutput -> CommittedPlan -> apply()`
//!
//! See ADR-003 for design rationale.

pub mod apply;
pub mod autonomous;
pub mod lint;
pub mod memory;
pub mod plan;

use serde::{Deserialize, Serialize};

/// Guardrails for a topology — safety constraints that block deployment if exceeded.
///
/// Placed inline in the `.wh` file under a `guardrails:` key.
/// All fields are optional; omitting the section entirely means no constraints.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Guardrails {
    /// Maximum allowed replicas for any single agent in this topology.
    #[serde(default)]
    pub max_replicas: Option<u32>,
}

/// Represents a parsed and validated `.wh` topology file.
///
/// A topology declares the desired state of agents, streams, and surfaces.
/// Uses `BTreeMap` for deterministic key ordering in serialization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Topology {
    pub api_version: String,
    pub name: String,
    #[serde(default)]
    pub agents: Vec<Agent>,
    #[serde(default)]
    pub streams: Vec<Stream>,
    /// Optional guardrails for safety constraints (e.g., max_replicas).
    #[serde(default)]
    pub guardrails: Option<Guardrails>,
}

/// An agent declaration within a topology.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Agent {
    pub name: String,
    pub image: String,
    #[serde(default = "default_replicas")]
    pub replicas: u32,
    #[serde(default)]
    pub streams: Vec<String>,
}

fn default_replicas() -> u32 {
    1
}

/// A stream declaration within a topology.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Stream {
    pub name: String,
    #[serde(default)]
    pub retention: Option<String>,
}

/// A single change detected between current and desired topology state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Change {
    /// Operation type: "+" (add), "-" (remove), "~" (modify)
    pub op: String,
    /// Component description, e.g. "agent researcher"
    pub component: String,
    /// Field that changed (for modifications)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Previous value (for modifications)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<serde_json::Value>,
    /// New value (for modifications)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<serde_json::Value>,
}

/// Errors that can occur during deploy operations.
#[derive(Debug, thiserror::Error)]
pub enum DeployError {
    #[error("failed to read topology file: {0}")]
    FileRead(#[from] std::io::Error),

    #[error("invalid topology file: {0}")]
    InvalidTopology(String),

    #[error("YAML parse error: {0}")]
    YamlParse(#[from] serde_yaml::Error),

    #[error("plan failed: {0}")]
    PlanFailed(String),

    #[error("apply failed: {0}")]
    ApplyFailed(String),

    #[error("git operation failed: {0}")]
    GitFailed(String),

    #[error("git operation timed out after {0}s")]
    GitTimeout(u64),

    #[error("policy violation: {0}")]
    PolicyViolation(String),

    #[error("self-destruct detected: {0}")]
    SelfDestructDetected(String),

    #[error("Podman not found: {0}")]
    PodmanNotFound(String),

    #[error("Podman operation failed: {0}")]
    PodmanFailed(String),
}

impl DeployError {
    /// Returns the error code string in SCREAMING_SNAKE_CASE (NP-01).
    pub fn code(&self) -> &'static str {
        match self {
            DeployError::FileRead(_) => "FILE_READ_ERROR",
            DeployError::InvalidTopology(_) => "INVALID_TOPOLOGY",
            DeployError::YamlParse(_) => "YAML_PARSE_ERROR",
            DeployError::PlanFailed(_) => "PLAN_FAILED",
            DeployError::ApplyFailed(_) => "APPLY_FAILED",
            DeployError::GitFailed(_) => "GIT_FAILED",
            DeployError::GitTimeout(_) => "GIT_TIMEOUT",
            DeployError::PolicyViolation(_) => "POLICY_VIOLATION",
            DeployError::SelfDestructDetected(_) => "SELF_DESTRUCT_DETECTED",
            DeployError::PodmanNotFound(_) => "PODMAN_NOT_FOUND",
            DeployError::PodmanFailed(_) => "PODMAN_FAILED",
        }
    }
}

/// Parse a `.wh` YAML file into a `Topology`.
pub fn parse_topology(content: &str) -> Result<Topology, DeployError> {
    let topology: Topology =
        serde_yaml::from_str(content).map_err(DeployError::YamlParse)?;

    // Validate api_version
    if topology.api_version != "wheelhouse.dev/v1" {
        return Err(DeployError::InvalidTopology(format!(
            "unsupported api_version: '{}', expected 'wheelhouse.dev/v1'",
            topology.api_version
        )));
    }

    if topology.name.is_empty() {
        return Err(DeployError::InvalidTopology(
            "topology name must not be empty".to_string(),
        ));
    }

    Ok(topology)
}

/// Load a topology from a `.wh` file path.
pub fn load_topology(path: &std::path::Path) -> Result<Topology, DeployError> {
    let content = std::fs::read_to_string(path)?;
    parse_topology(&content)
}

/// Canonicalize a topology for deterministic comparison.
/// Sorts agents and streams by name for consistent diffing.
pub fn canonicalize_topology(mut topology: Topology) -> Topology {
    topology.agents.sort();
    topology.streams.sort();
    topology
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_topology() {
        let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
agents:
  - name: researcher
    image: researcher:latest
    replicas: 2
    streams:
      - main
streams:
  - name: main
    retention: 7d
"#;
        let topo = parse_topology(yaml).unwrap();
        assert_eq!(topo.name, "dev");
        assert_eq!(topo.agents.len(), 1);
        assert_eq!(topo.agents[0].replicas, 2);
        assert_eq!(topo.streams.len(), 1);
    }

    #[test]
    fn parse_invalid_api_version() {
        let yaml = r#"
api_version: wheelhouse.dev/v99
name: dev
"#;
        let err = parse_topology(yaml).unwrap_err();
        assert!(matches!(err, DeployError::InvalidTopology(_)));
    }

    #[test]
    fn parse_empty_name_fails() {
        let yaml = r#"
api_version: wheelhouse.dev/v1
name: ""
"#;
        let err = parse_topology(yaml).unwrap_err();
        assert!(matches!(err, DeployError::InvalidTopology(_)));
    }

    #[test]
    fn parse_malformed_yaml_fails() {
        let yaml = "not: [valid: yaml: {{{";
        let err = parse_topology(yaml).unwrap_err();
        assert!(matches!(err, DeployError::YamlParse(_)));
    }

    #[test]
    fn default_replicas_is_one() {
        let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
agents:
  - name: researcher
    image: researcher:latest
"#;
        let topo = parse_topology(yaml).unwrap();
        assert_eq!(topo.agents[0].replicas, 1);
    }

    #[test]
    fn canonicalize_sorts_agents_and_streams() {
        let topo = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "dev".to_string(),
            agents: vec![
                Agent {
                    name: "zeta".to_string(),
                    image: "z:latest".to_string(),
                    replicas: 1,
                    streams: vec![],
                },
                Agent {
                    name: "alpha".to_string(),
                    image: "a:latest".to_string(),
                    replicas: 1,
                    streams: vec![],
                },
            ],
            streams: vec![
                Stream {
                    name: "z-stream".to_string(),
                    retention: None,
                },
                Stream {
                    name: "a-stream".to_string(),
                    retention: None,
                },
            ],
            guardrails: None,
        };
        let canonical = canonicalize_topology(topo);
        assert_eq!(canonical.agents[0].name, "alpha");
        assert_eq!(canonical.streams[0].name, "a-stream");
    }
}
