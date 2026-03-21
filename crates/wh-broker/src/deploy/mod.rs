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

/// Broker configuration within a topology (ADR-029).
///
/// When present in the `.wh` file, the broker runs as a container with the
/// specified image and port mappings. When absent, the native process fallback
/// is used (deprecated — `wh topology lint` emits a warning).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrokerSpec {
    /// Container image for the broker (e.g., `ghcr.io/wheelhouse-paris/wh-broker:latest`).
    pub image: String,
    /// Port mappings published on the host (e.g., `["127.0.0.1:5555:5555"]`).
    /// Defaults to the standard broker ports if empty or absent.
    #[serde(default)]
    pub ports: Vec<String>,
}

/// Represents a parsed and validated `.wh` topology file.
///
/// A topology declares the desired state of agents, streams, and surfaces.
/// Uses `BTreeMap` for deterministic key ordering in serialization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Topology {
    pub api_version: String,
    pub name: String,
    /// Optional broker configuration (ADR-029). When absent, native process
    /// fallback is used (deprecated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broker: Option<BrokerSpec>,
    /// Optional path or URL to the git repository containing skills.
    /// Topology-level: shared across all agents in this topology.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills_repo: Option<String>,
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
    /// Optional list of skills available to this agent (FM-05 allowlist).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<SkillRefConfig>>,
    /// Whether this agent can create/modify `.wh` files and run `wh topology apply` (ADR-034).
    /// Defaults to `false`. Must be declared in the `.wh` spec — not configurable at runtime (E12-13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology_edit: Option<bool>,
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
    /// Optional human-readable description. When present, `wh topology apply` creates
    /// `.wh/context/<stream_name>/CONTEXT.md` with this content (FR-NEW-04).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A surface declaration within a topology.
///
/// Surfaces connect external channels (Telegram, CLI, etc.) to streams.
/// Each surface is provisioned as a Podman container on the topology network
/// (ADR-026). The container image is derived from `kind`:
/// `kind: telegram` -> `ghcr.io/wheelhouse-paris/wh-telegram:latest`.
/// CLI surfaces are the exception and remain native (no container).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Surface {
    pub name: String,
    /// Surface type: "telegram" or "cli".
    pub kind: String,
    /// Stream name this surface connects to (single-stream mode).
    /// Mutually exclusive with `chats`.
    #[serde(default)]
    pub stream: String,
    /// Optional environment variables passed to the surface process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::BTreeMap<String, String>>,
    /// Multi-chat configuration (Telegram surfaces only).
    /// Each entry maps a chat (DM or supergroup) to one or more streams.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chats: Option<Vec<SurfaceChatConfig>>,
}

/// A chat entry within a surface's `chats` block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SurfaceChatConfig {
    /// Chat identifier: `@username` for DMs, or group display name for supergroups.
    pub id: String,
    /// Stream name for DM chats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<String>,
    /// Thread (topic) list for supergroup chats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads: Option<Vec<SurfaceThreadConfig>>,
}

/// A thread/topic entry within a chat configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SurfaceThreadConfig {
    /// Topic name (human-readable).
    pub id: String,
    /// Stream name this topic is bridged to.
    pub stream: String,
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
    /// Source `.wh` file this component originated from (E12-03, folder-based composition).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_file: Option<String>,
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

    #[error("topology edit denied: {0}")]
    TopologyEditDenied(String),
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
            DeployError::TopologyEditDenied(_) => "TOPOLOGY_EDIT_DENIED",
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

/// Load a topology from a path that may be a single `.wh` file or a folder
/// containing multiple `.wh` files (ADR-030).
///
/// When `path` is a directory, all `*.wh` files are discovered, parsed
/// independently, and merged into a single `Topology` in lexicographic order
/// by filename. Duplicate agent/stream/surface names across files are errors.
///
/// Returns the merged topology and a mapping of component names to source files.
pub fn load_topology_from_path(
    path: &std::path::Path,
) -> Result<(Topology, ComponentSourceMap), DeployError> {
    if path.is_dir() {
        load_topology_folder(path)
    } else {
        let topo = load_topology(path)?;
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        let source_map = ComponentSourceMap::from_topology(&topo, &filename);
        Ok((topo, source_map))
    }
}

/// Tracks which source file each component (agent/stream/surface) came from.
#[derive(Debug, Clone, Default)]
pub struct ComponentSourceMap {
    /// Maps component name -> source filename (e.g., "researcher" -> "agents.wh")
    pub entries: std::collections::BTreeMap<String, String>,
}

impl ComponentSourceMap {
    /// Build a source map from a single topology and filename.
    pub fn from_topology(topo: &Topology, filename: &str) -> Self {
        let mut entries = std::collections::BTreeMap::new();
        for agent in &topo.agents {
            entries.insert(format!("agent:{}", agent.name), filename.to_string());
        }
        for stream in &topo.streams {
            entries.insert(format!("stream:{}", stream.name), filename.to_string());
        }
        for surface in &topo.surfaces {
            entries.insert(format!("surface:{}", surface.name), filename.to_string());
        }
        Self { entries }
    }

    /// Look up the source file for a component.
    pub fn source_file(&self, component_key: &str) -> Option<&str> {
        self.entries.get(component_key).map(|s| s.as_str())
    }
}

/// Discover all `*.wh` files in a folder, parse each, merge into a single
/// `Topology` (ADR-030). Files are processed in lexicographic order by filename.
///
/// Errors on:
/// - No `.wh` files found in folder
/// - apiVersion mismatch across files (E12-01)
/// - Topology name mismatch across files
/// - Duplicate agent/stream/surface names across files
/// - Multiple files declaring `broker` section
/// - Multiple files declaring `guardrails` section
fn load_topology_folder(
    dir: &std::path::Path,
) -> Result<(Topology, ComponentSourceMap), DeployError> {
    // Discover *.wh files
    let mut wh_files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_file() && path.extension().is_some_and(|ext| ext == "wh") {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    if wh_files.is_empty() {
        return Err(DeployError::InvalidTopology(format!(
            "no .wh files found in folder '{}'",
            dir.display()
        )));
    }

    // Sort lexicographically by filename (not full path)
    wh_files.sort_by(|a, b| {
        a.file_name()
            .unwrap_or_default()
            .cmp(b.file_name().unwrap_or_default())
    });

    // Parse each file
    let mut topologies: Vec<(String, Topology)> = Vec::new();
    for path in &wh_files {
        let topo = load_topology(path)?;
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        topologies.push((filename, topo));
    }

    // Validate apiVersion consistency (E12-01)
    let first_version = &topologies[0].1.api_version;
    for (filename, topo) in &topologies[1..] {
        if &topo.api_version != first_version {
            return Err(DeployError::InvalidTopology(format!(
                "apiVersion mismatch: '{}' declares '{}' but '{}' declares '{}'",
                topologies[0].0, first_version, filename, topo.api_version
            )));
        }
    }

    // Validate topology name consistency
    let first_name = &topologies[0].1.name;
    for (filename, topo) in &topologies[1..] {
        if &topo.name != first_name {
            return Err(DeployError::InvalidTopology(format!(
                "topology name mismatch: '{}' declares '{}' but '{}' declares '{}'",
                topologies[0].0, first_name, filename, topo.name
            )));
        }
    }

    // Build merged topology and detect duplicates
    let mut merged = Topology {
        api_version: first_version.clone(),
        name: first_name.clone(),
        broker: None,
        skills_repo: None,
        agents: Vec::new(),
        streams: Vec::new(),
        surfaces: Vec::new(),
        guardrails: None,
    };

    let mut source_map = ComponentSourceMap::default();
    let mut seen_agents: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    let mut seen_streams: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    let mut seen_surfaces: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    let mut broker_file: Option<String> = None;
    let mut skills_repo_file: Option<String> = None;
    let mut guardrails_file: Option<String> = None;

    for (filename, topo) in topologies {
        // Check broker conflicts
        if topo.broker.is_some() {
            if let Some(ref prev_file) = broker_file {
                return Err(DeployError::InvalidTopology(format!(
                    "conflicting broker sections: declared in both '{prev_file}' and '{filename}'"
                )));
            }
            broker_file = Some(filename.clone());
            merged.broker = topo.broker;
        }

        // Check skills_repo conflicts
        if topo.skills_repo.is_some() {
            if let Some(ref prev_file) = skills_repo_file {
                return Err(DeployError::InvalidTopology(format!(
                    "conflicting skills_repo: declared in both '{prev_file}' and '{filename}'"
                )));
            }
            skills_repo_file = Some(filename.clone());
            merged.skills_repo = topo.skills_repo;
        }

        // Check guardrails conflicts
        if topo.guardrails.is_some() {
            if let Some(ref prev_file) = guardrails_file {
                return Err(DeployError::InvalidTopology(format!(
                    "conflicting guardrails sections: declared in both '{prev_file}' and '{filename}'"
                )));
            }
            guardrails_file = Some(filename.clone());
            merged.guardrails = topo.guardrails;
        }

        // Merge agents with duplicate detection
        for agent in topo.agents {
            if let Some(prev_file) = seen_agents.get(&agent.name) {
                return Err(DeployError::InvalidTopology(format!(
                    "duplicate agent name '{}' across files: '{}' and '{}'",
                    agent.name, prev_file, filename
                )));
            }
            seen_agents.insert(agent.name.clone(), filename.clone());
            source_map
                .entries
                .insert(format!("agent:{}", agent.name), filename.clone());
            merged.agents.push(agent);
        }

        // Merge streams with duplicate detection
        for stream in topo.streams {
            if let Some(prev_file) = seen_streams.get(&stream.name) {
                return Err(DeployError::InvalidTopology(format!(
                    "duplicate stream name '{}' across files: '{}' and '{}'",
                    stream.name, prev_file, filename
                )));
            }
            seen_streams.insert(stream.name.clone(), filename.clone());
            source_map
                .entries
                .insert(format!("stream:{}", stream.name), filename.clone());
            merged.streams.push(stream);
        }

        // Merge surfaces with duplicate detection
        for surface in topo.surfaces {
            if let Some(prev_file) = seen_surfaces.get(&surface.name) {
                return Err(DeployError::InvalidTopology(format!(
                    "duplicate surface name '{}' across files: '{}' and '{}'",
                    surface.name, prev_file, filename
                )));
            }
            seen_surfaces.insert(surface.name.clone(), filename.clone());
            source_map
                .entries
                .insert(format!("surface:{}", surface.name), filename.clone());
            merged.surfaces.push(surface);
        }
    }

    Ok((merged, source_map))
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
            broker: None,
            skills_repo: None,
            agents: vec![
                Agent {
                    name: "zeta".to_string(),
                    image: "z:latest".to_string(),
                    replicas: 1,
                    streams: vec![],
                    persona: None,

                    skills: None,
                    topology_edit: None,
                },
                Agent {
                    name: "alpha".to_string(),
                    image: "a:latest".to_string(),
                    replicas: 1,
                    streams: vec![],
                    persona: None,

                    skills: None,
                    topology_edit: None,
                },
            ],
            streams: vec![
                Stream {
                    name: "z-stream".to_string(),
                    retention: None,
                    description: None,
                },
                Stream {
                    name: "a-stream".to_string(),
                    retention: None,
                    description: None,
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
skills_repo: /path/to/skills
agents:
  - name: donna
    image: agent-claude:latest
    streams: [main]
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
            topo.skills_repo,
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
        assert_eq!(topo.skills_repo, None);
        assert_eq!(topo.agents[0].skills, None);
    }

    #[test]
    fn skills_yaml_roundtrip() {
        let topo = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "dev".to_string(),
            broker: None,
            skills_repo: Some("/skills".to_string()),
            agents: vec![Agent {
                name: "donna".to_string(),
                image: "agent-claude:latest".to_string(),
                replicas: 1,
                streams: vec!["main".to_string()],
                persona: None,
                skills: Some(vec![SkillRefConfig {
                    name: "summarize".to_string(),
                    version: "1.0.0".to_string(),
                }]),
                topology_edit: None,
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
            broker: None,
            skills_repo: None,
            agents: vec![],
            streams: vec![],
            surfaces: vec![
                Surface {
                    name: "zeta-surface".to_string(),
                    kind: "cli".to_string(),
                    stream: "main".to_string(),
                    env: None,
                    chats: None,
                },
                Surface {
                    name: "alpha-surface".to_string(),
                    kind: "telegram".to_string(),
                    stream: "main".to_string(),
                    env: None,
                    chats: None,
                },
            ],
            guardrails: None,
        };
        let canonical = canonicalize_topology(topo);
        assert_eq!(canonical.surfaces[0].name, "alpha-surface");
        assert_eq!(canonical.surfaces[1].name, "zeta-surface");
    }

    #[test]
    fn parse_topology_with_stream_description() {
        let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
streams:
  - name: main
    description: "Main conversation stream"
"#;
        let topo = parse_topology(yaml).unwrap();
        assert_eq!(
            topo.streams[0].description,
            Some("Main conversation stream".to_string())
        );
    }

    #[test]
    fn parse_topology_without_stream_description_defaults_to_none() {
        let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
streams:
  - name: main
    retention: 7d
"#;
        let topo = parse_topology(yaml).unwrap();
        assert_eq!(topo.streams[0].description, None);
    }

    #[test]
    fn stream_description_yaml_roundtrip() {
        let topo = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "dev".to_string(),
            broker: None,
            skills_repo: None,
            agents: vec![],
            streams: vec![Stream {
                name: "main".to_string(),
                retention: None,
                description: Some("Test context".to_string()),
            }],
            surfaces: vec![],
            guardrails: None,
        };
        let yaml = serde_yaml::to_string(&topo).unwrap();
        let parsed: Topology = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(
            parsed.streams[0].description,
            Some("Test context".to_string())
        );
    }

    #[test]
    fn stream_description_none_not_serialized() {
        let topo = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "dev".to_string(),
            broker: None,
            skills_repo: None,
            agents: vec![],
            streams: vec![Stream {
                name: "main".to_string(),
                retention: None,
                description: None,
            }],
            surfaces: vec![],
            guardrails: None,
        };
        let yaml = serde_yaml::to_string(&topo).unwrap();
        assert!(
            !yaml.contains("description"),
            "description: None should be omitted from YAML: {yaml}"
        );
    }

    #[test]
    fn parse_topology_with_broker_section() {
        let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
broker:
  image: ghcr.io/wheelhouse-paris/wh-broker:latest
  ports:
    - "127.0.0.1:5555:5555"
    - "127.0.0.1:5556:5556"
    - "127.0.0.1:5557:5557"
agents:
  - name: researcher
    image: researcher:latest
streams:
  - name: main
"#;
        let topo = parse_topology(yaml).unwrap();
        let broker = topo.broker.as_ref().unwrap();
        assert_eq!(broker.image, "ghcr.io/wheelhouse-paris/wh-broker:latest");
        assert_eq!(broker.ports.len(), 3);
        assert_eq!(broker.ports[0], "127.0.0.1:5555:5555");
    }

    #[test]
    fn parse_topology_without_broker_section() {
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
        assert!(topo.broker.is_none());
    }

    #[test]
    fn parse_topology_broker_without_ports_defaults_empty() {
        let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
broker:
  image: ghcr.io/wheelhouse-paris/wh-broker:v1.0.0
"#;
        let topo = parse_topology(yaml).unwrap();
        let broker = topo.broker.as_ref().unwrap();
        assert_eq!(broker.image, "ghcr.io/wheelhouse-paris/wh-broker:v1.0.0");
        assert!(broker.ports.is_empty());
    }

    #[test]
    fn broker_yaml_roundtrip() {
        let topo = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "dev".to_string(),
            broker: Some(BrokerSpec {
                image: "ghcr.io/wheelhouse-paris/wh-broker:latest".to_string(),
                ports: vec![
                    "127.0.0.1:5555:5555".to_string(),
                    "127.0.0.1:5556:5556".to_string(),
                ],
            }),
            skills_repo: None,
            agents: vec![],
            streams: vec![],
            surfaces: vec![],
            guardrails: None,
        };
        let yaml = serde_yaml::to_string(&topo).unwrap();
        let parsed: Topology = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(topo, parsed);
    }

    #[test]
    fn broker_none_not_serialized() {
        let topo = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "dev".to_string(),
            broker: None,
            skills_repo: None,
            agents: vec![],
            streams: vec![],
            surfaces: vec![],
            guardrails: None,
        };
        let yaml = serde_yaml::to_string(&topo).unwrap();
        assert!(
            !yaml.contains("broker"),
            "broker: None should be omitted from YAML: {yaml}"
        );
    }

    #[test]
    fn parse_topology_with_topology_edit_true() {
        let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
agents:
  - name: donna
    image: agent-claude:latest
    topology_edit: true
"#;
        let topo = parse_topology(yaml).unwrap();
        assert_eq!(topo.agents[0].topology_edit, Some(true));
    }

    #[test]
    fn parse_topology_without_topology_edit_defaults_to_none() {
        let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
agents:
  - name: researcher
    image: researcher:latest
"#;
        let topo = parse_topology(yaml).unwrap();
        assert_eq!(topo.agents[0].topology_edit, None);
    }

    #[test]
    fn topology_edit_false_explicit() {
        let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
agents:
  - name: researcher
    image: researcher:latest
    topology_edit: false
"#;
        let topo = parse_topology(yaml).unwrap();
        assert_eq!(topo.agents[0].topology_edit, Some(false));
    }

    #[test]
    fn topology_edit_yaml_roundtrip() {
        let topo = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "dev".to_string(),
            broker: None,
            skills_repo: None,
            agents: vec![Agent {
                name: "donna".to_string(),
                image: "agent-claude:latest".to_string(),
                replicas: 1,
                streams: vec!["main".to_string()],
                persona: None,
                skills: None,
                topology_edit: Some(true),
            }],
            streams: vec![],
            surfaces: vec![],
            guardrails: None,
        };
        let yaml = serde_yaml::to_string(&topo).unwrap();
        assert!(
            yaml.contains("topology_edit: true"),
            "YAML should contain topology_edit: {yaml}"
        );
        let parsed: Topology = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.agents[0].topology_edit, Some(true));
    }

    #[test]
    fn topology_edit_none_not_serialized() {
        let topo = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "dev".to_string(),
            broker: None,
            skills_repo: None,
            agents: vec![Agent {
                name: "donna".to_string(),
                image: "agent-claude:latest".to_string(),
                replicas: 1,
                streams: vec![],
                persona: None,
                skills: None,
                topology_edit: None,
            }],
            streams: vec![],
            surfaces: vec![],
            guardrails: None,
        };
        let yaml = serde_yaml::to_string(&topo).unwrap();
        assert!(
            !yaml.contains("topology_edit"),
            "topology_edit: None should be omitted from YAML: {yaml}"
        );
    }

    #[test]
    fn topology_edit_denied_error_code() {
        let err = DeployError::TopologyEditDenied("test".to_string());
        assert_eq!(err.code(), "TOPOLOGY_EDIT_DENIED");
    }
}
