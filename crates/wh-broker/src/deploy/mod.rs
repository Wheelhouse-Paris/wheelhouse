//! Deploy pipeline: lint -> plan -> commit -> apply
//!
//! Implements the typestate pattern (5W-03) where the compiler enforces
//! the correct ordering of deploy operations:
//! `LintedFile -> PlanOutput -> CommittedPlan -> apply()`
//!
//! See ADR-003 for design rationale.

pub mod apply;
pub mod approval;
pub mod autonomous;
pub mod gitignore;
pub mod lint;
pub mod memory;
pub mod persona;
pub mod plan;
pub mod podman;

use serde::{Deserialize, Serialize};

/// Threshold level for autonomous apply — determines which impact levels require human approval.
///
/// - `Low`: only low-impact changes proceed automatically; medium and high require approval.
/// - `Medium`: low and medium-impact changes proceed; high requires approval.
/// - `High`: all changes proceed automatically (effectively disables threshold).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThresholdLevel {
    Low,
    Medium,
    High,
}

/// Guardrails for a topology — safety constraints that block deployment if exceeded.
///
/// Placed inline in the `.wh` file under a `guardrails:` key.
/// All fields are optional; omitting the section entirely means no constraints.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Guardrails {
    /// Maximum allowed replicas for any single agent in this topology.
    #[serde(default)]
    pub max_replicas: Option<u32>,
    /// Threshold level for autonomous apply — determines which impact levels
    /// require human approval before applying. `None` = all changes proceed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomous_apply_threshold: Option<ThresholdLevel>,
    /// Timeout in seconds for pending human approval requests.
    /// Default: 86400 (24 hours).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_timeout_secs: Option<u64>,
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
    /// Surface declarations (Telegram, CLI, etc.).
    #[serde(default)]
    pub surfaces: Vec<Surface>,
    /// Optional guardrails for safety constraints (e.g., max_replicas).
    #[serde(default)]
    pub guardrails: Option<Guardrails>,
}

/// A skill reference in an agent's configuration.
///
/// Mirrors `wh_skill::config::SkillRef` but stays in the deploy layer.
/// Converted to `wh_skill::config::SkillRef` at registration time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SkillRefConfig {
    /// Skill name (must match a directory in the skills repo).
    pub name: String,
    /// Version pin: bare semver, `"branch:<name>"`, or `"commit:<sha>"`.
    pub version: String,
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
    /// Optional path to persona files directory (e.g., `agents/donna/`).
    /// Contains SOUL.md, IDENTITY.md, and MEMORY.md.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona: Option<String>,
    /// Optional path to the git repository containing skills.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills_repo: Option<String>,
    /// Optional list of skills available to this agent (FM-05 allowlist).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<SkillRefConfig>>,
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

/// A surface declaration within a topology.
///
/// Surfaces connect external channels (Telegram, CLI, etc.) to streams.
/// Each surface is provisioned as a native process (not a container).
/// The binary is resolved from `kind`: `kind: telegram` → `wh-telegram`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Surface {
    pub name: String,
    /// Surface type: "telegram" or "cli".
    pub kind: String,
    /// Stream name this surface connects to.
    pub stream: String,
    /// Optional environment variables passed to the surface process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::BTreeMap<String, String>>,
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

    #[error("persona load failed: {0}")]
    PersonaLoadFailed(String),

    #[error("secrets detected in staged files: {0:?}")]
    SecretsDetected(Vec<String>),

    #[error("approval required: {0}")]
    ApprovalRequired(String),
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
            DeployError::PersonaLoadFailed(_) => "PERSONA_LOAD_FAILED",
            DeployError::SecretsDetected(_) => "SECRETS_DETECTED",
            DeployError::ApprovalRequired(_) => "APPROVAL_REQUIRED",
        }
    }
}

/// Parse a `.wh` YAML file into a `Topology`.
pub fn parse_topology(content: &str) -> Result<Topology, DeployError> {
    let topology: Topology = serde_yaml::from_str(content).map_err(DeployError::YamlParse)?;

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
    topology.surfaces.sort();
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
                    persona: None,
                    skills_repo: None,
                    skills: None,
                },
                Agent {
                    name: "alpha".to_string(),
                    image: "a:latest".to_string(),
                    replicas: 1,
                    streams: vec![],
                    persona: None,
                    skills_repo: None,
                    skills: None,
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
            surfaces: vec![],
            guardrails: None,
        };
        let canonical = canonicalize_topology(topo);
        assert_eq!(canonical.agents[0].name, "alpha");
        assert_eq!(canonical.streams[0].name, "a-stream");
    }

    #[test]
    fn parse_topology_with_persona_field() {
        let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
agents:
  - name: donna
    image: agent-claude:latest
    streams: [main]
    persona: agents/donna/
streams:
  - name: main
"#;
        let topo = parse_topology(yaml).unwrap();
        assert_eq!(topo.agents[0].persona, Some("agents/donna/".to_string()));
    }

    #[test]
    fn parse_topology_without_persona_defaults_to_none() {
        let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
agents:
  - name: researcher
    image: researcher:latest
streams:
  - name: main
"#;
        let topo = parse_topology(yaml).unwrap();
        assert_eq!(topo.agents[0].persona, None);
    }

    #[test]
    fn persona_load_failed_error_code() {
        let err = DeployError::PersonaLoadFailed("test".to_string());
        assert_eq!(err.code(), "PERSONA_LOAD_FAILED");
    }

    #[test]
    fn parse_topology_with_skills_config() {
        let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
agents:
  - name: donna
    image: agent-claude:latest
    streams: [main]
    skills_repo: /path/to/skills
    skills:
      - name: summarize
        version: "1.0.0"
      - name: web-search
        version: "branch:main"
streams:
  - name: main
"#;
        let topo = parse_topology(yaml).unwrap();
        assert_eq!(
            topo.agents[0].skills_repo,
            Some("/path/to/skills".to_string())
        );
        let skills = topo.agents[0].skills.as_ref().unwrap();
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "summarize");
        assert_eq!(skills[0].version, "1.0.0");
        assert_eq!(skills[1].name, "web-search");
        assert_eq!(skills[1].version, "branch:main");
    }

    #[test]
    fn parse_topology_without_skills_defaults_to_none() {
        let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
agents:
  - name: researcher
    image: researcher:latest
streams:
  - name: main
"#;
        let topo = parse_topology(yaml).unwrap();
        assert_eq!(topo.agents[0].skills_repo, None);
        assert_eq!(topo.agents[0].skills, None);
    }

    #[test]
    fn skills_yaml_roundtrip() {
        let topo = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "dev".to_string(),
            agents: vec![Agent {
                name: "donna".to_string(),
                image: "agent-claude:latest".to_string(),
                replicas: 1,
                streams: vec!["main".to_string()],
                persona: None,
                skills_repo: Some("/skills".to_string()),
                skills: Some(vec![SkillRefConfig {
                    name: "summarize".to_string(),
                    version: "1.0.0".to_string(),
                }]),
            }],
            streams: vec![],
            surfaces: vec![],
            guardrails: None,
        };
        let yaml = serde_yaml::to_string(&topo).unwrap();
        let parsed: Topology = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(topo, parsed);
    }

    #[test]
    fn parse_topology_with_surfaces() {
        let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
agents:
  - name: researcher
    image: researcher:latest
streams:
  - name: main
surfaces:
  - name: telegram
    kind: telegram
    stream: main
  - name: cli
    kind: cli
    stream: main
"#;
        let topo = parse_topology(yaml).unwrap();
        assert_eq!(topo.surfaces.len(), 2);
        assert_eq!(topo.surfaces[0].name, "telegram");
        assert_eq!(topo.surfaces[0].kind, "telegram");
        assert_eq!(topo.surfaces[0].stream, "main");
        assert_eq!(topo.surfaces[1].name, "cli");
    }

    #[test]
    fn parse_topology_with_surface_env() {
        let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
surfaces:
  - name: telegram
    kind: telegram
    stream: main
    env:
      TELEGRAM_BOT_TOKEN: "tok123"
      CHAT_ID: "456"
"#;
        let topo = parse_topology(yaml).unwrap();
        let env = topo.surfaces[0].env.as_ref().unwrap();
        assert_eq!(env.get("TELEGRAM_BOT_TOKEN").unwrap(), "tok123");
        assert_eq!(env.get("CHAT_ID").unwrap(), "456");
    }

    #[test]
    fn parse_topology_without_surfaces_defaults_empty() {
        let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
agents:
  - name: researcher
    image: researcher:latest
"#;
        let topo = parse_topology(yaml).unwrap();
        assert!(topo.surfaces.is_empty());
    }

    #[test]
    fn canonicalize_sorts_surfaces() {
        let topo = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "dev".to_string(),
            agents: vec![],
            streams: vec![],
            surfaces: vec![
                Surface {
                    name: "zeta-surface".to_string(),
                    kind: "cli".to_string(),
                    stream: "main".to_string(),
                    env: None,
                },
                Surface {
                    name: "alpha-surface".to_string(),
                    kind: "telegram".to_string(),
                    stream: "main".to_string(),
                    env: None,
                },
            ],
            guardrails: None,
        };
        let canonical = canonicalize_topology(topo);
        assert_eq!(canonical.surfaces[0].name, "alpha-surface");
        assert_eq!(canonical.surfaces[1].name, "zeta-surface");
    }
}
