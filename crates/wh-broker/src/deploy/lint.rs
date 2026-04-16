//! Lint step of the deploy pipeline.
//!
//! Produces a `LintedFile` typestate token that must be consumed by `plan()`.
//! This enforces the FM-03 transaction order at compile time (5W-03).

use std::path::{Path, PathBuf};

use std::collections::BTreeMap;

use crate::deploy::composition;
use crate::deploy::podman;
use crate::deploy::{
    load_topology, load_topology_from_path, ComponentSourceMap, DeployError, Topology,
};

/// A successfully linted topology file (or folder).
///
/// This typestate token proves that the topology file has been parsed and
/// validated. It must be consumed by `plan()` — the compiler enforces this
/// via `#[must_use]`.
#[must_use = "a LintedFile must be passed to plan() — do not discard"]
#[derive(Debug)]
pub struct LintedFile {
    pub(crate) topology: Topology,
    pub(crate) source_path: PathBuf,
    /// Component-to-source-file mapping (populated for folder-based composition).
    pub(crate) source_map: ComponentSourceMap,
}

impl LintedFile {
    /// Returns a reference to the parsed topology.
    pub fn topology(&self) -> &Topology {
        &self.topology
    }

    /// Returns the source file path.
    pub fn source_path(&self) -> &Path {
        &self.source_path
    }

    /// Returns the component source map (E12-03).
    pub fn source_map(&self) -> &ComponentSourceMap {
        &self.source_map
    }
}

/// Lint a `.wh` topology path (file or folder): parse, validate structure,
/// and return a `LintedFile` token.
///
/// When `path` is a directory, all `*.wh` files are discovered, parsed
/// independently, merged, and validated for cross-file consistency (ADR-030).
///
/// This is the entry point of the deploy pipeline typestate chain.
///
/// **Known limitation**: This broker-side lint only validates YAML structure via serde
/// deserialization (`parse_topology()`). It does NOT validate field-level rules for
/// surfaces (e.g., `kind` must be "telegram"/"cli", `stream` must reference a declared
/// stream). Those validations live in the CLI lint layer (`wh-cli/src/lint.rs:validate_surfaces()`).
/// If the broker is called directly (not via CLI), surfaces with invalid field values
/// will be accepted. This mirrors the pre-existing pattern for agents and streams.
#[tracing::instrument(skip_all, fields(path = %path.as_ref().display()))]
#[must_use = "a LintedFile must be passed to plan() — do not discard"]
pub fn lint(path: impl AsRef<Path>) -> Result<LintedFile, DeployError> {
    let path = path.as_ref();

    if path.is_dir() {
        let (topology, source_map) = load_topology_from_path(path)?;
        // Expand subsystem declarations before validation (ADR-045)
        let topology = composition::expand_subsystems(topology)?;
        // Validate volume mounts after subsystem expansion (ADR-043, ADR-044)
        validate_volume_mounts(&topology)?;
        Ok(LintedFile {
            topology,
            source_path: path.to_path_buf(),
            source_map,
        })
    } else {
        let topology = load_topology(path)?;
        // Expand subsystem declarations before validation (ADR-045)
        let topology = composition::expand_subsystems(topology)?;
        // Validate volume mounts after subsystem expansion (ADR-043, ADR-044)
        validate_volume_mounts(&topology)?;
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        let source_map = ComponentSourceMap::from_topology(&topology, &filename);
        Ok(LintedFile {
            topology,
            source_path: path.to_path_buf(),
            source_map,
        })
    }
}

/// Validate volume mounts on the expanded topology (ADR-043, ADR-044).
///
/// Runs after subsystem expansion so that generated volume mounts are included.
///
/// Checks:
/// 1. **Duplicate volume names**: subsystem-generated volumes must not collide
///    with standard topology volumes (wal, users, skills, personas, context, platform)
///    or per-agent workspace volumes.
/// 2. **Single-writer invariant**: at most one agent may mount a given volume as RW.
///    If more than one agent has an RW mount on the same volume, returns an error.
/// 3. **No-writer warning**: if a volume has zero RW mounts, logs a warning via tracing
///    (not a hard error — read-only shared volumes are a valid use case in some contexts,
///    but unexpected for the LLM-Wiki subsystem).
fn validate_volume_mounts(topology: &Topology) -> Result<(), DeployError> {
    // Collect standard topology volume names for collision detection.
    let agent_names: Vec<&str> = topology.agents.iter().map(|a| a.name.as_str()).collect();
    let standard_volumes: std::collections::HashSet<String> =
        podman::volume_names(&topology.name, &agent_names)
            .into_iter()
            .collect();

    // Collect all agent volume mounts: (volume_name -> Vec<(agent_name, mount_mode)>).
    let mut volume_agents: BTreeMap<String, Vec<(&str, &str)>> = BTreeMap::new();

    for agent in &topology.agents {
        for vol in &agent.volumes {
            // Check collision with standard volumes
            if standard_volumes.contains(&vol.name) {
                return Err(DeployError::InvalidTopology(format!(
                    "Duplicate volume name: '{}' collides with a standard topology volume",
                    vol.name
                )));
            }

            let mode = vol.mount_mode.as_deref().unwrap_or("rw");
            volume_agents
                .entry(vol.name.clone())
                .or_default()
                .push((&agent.name, mode));
        }
    }

    // Check single-writer invariant on each volume.
    for (vol_name, mounts) in &volume_agents {
        let rw_agents: Vec<&str> = mounts
            .iter()
            .filter(|(_, mode)| *mode == "rw")
            .map(|(name, _)| *name)
            .collect();

        if rw_agents.len() > 1 {
            return Err(DeployError::InvalidTopology(format!(
                "Volume '{}' has multiple rw mounts ({}) \u{2014} single-writer invariant violated",
                vol_name,
                rw_agents.join(", ")
            )));
        }

        if rw_agents.is_empty() {
            tracing::warn!(
                volume = %vol_name,
                "Volume has no rw mount \u{2014} no agent can write to it"
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn lint_valid_file_returns_linted_file() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(
            tmp,
            "api_version: wheelhouse.dev/v1\nname: dev\nagents: []\nstreams: []\n"
        )
        .unwrap();

        let linted = lint(tmp.path()).unwrap();
        assert_eq!(linted.topology().name, "dev");
    }

    #[test]
    fn lint_missing_file_returns_error() {
        let result = lint("/nonexistent/path/topology.wh");
        assert!(result.is_err());
    }

    #[test]
    fn lint_invalid_yaml_returns_error() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(tmp, "not valid yaml: {{{{").unwrap();

        let result = lint(tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn lint_folder_merges_multiple_files() {
        let dir = tempfile::tempdir().unwrap();

        // File 1: base topology with agents
        std::fs::write(
            dir.path().join("01-base.wh"),
            "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: researcher:latest\n",
        )
        .unwrap();

        // File 2: streams
        std::fs::write(
            dir.path().join("02-streams.wh"),
            "api_version: wheelhouse.dev/v1\nname: dev\nstreams:\n  - name: main\n",
        )
        .unwrap();

        let linted = lint(dir.path()).unwrap();
        assert_eq!(linted.topology().name, "dev");
        assert_eq!(linted.topology().agents.len(), 1);
        assert_eq!(linted.topology().streams.len(), 1);
        assert_eq!(linted.topology().agents[0].name, "researcher");
        assert_eq!(linted.topology().streams[0].name, "main");
    }

    #[test]
    fn lint_folder_detects_duplicate_agent_names() {
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(
            dir.path().join("a.wh"),
            "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\n",
        )
        .unwrap();

        std::fs::write(
            dir.path().join("b.wh"),
            "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r2:latest\n",
        )
        .unwrap();

        let err = lint(dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("duplicate agent name 'researcher'"),
            "got: {msg}"
        );
    }

    #[test]
    fn lint_folder_detects_api_version_mismatch() {
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(
            dir.path().join("a.wh"),
            "api_version: wheelhouse.dev/v1\nname: dev\n",
        )
        .unwrap();

        // This will fail at parse_topology level since v2 is unsupported,
        // but the error message should indicate the problem.
        std::fs::write(
            dir.path().join("b.wh"),
            "api_version: wheelhouse.dev/v2\nname: dev\n",
        )
        .unwrap();

        let err = lint(dir.path()).unwrap_err();
        assert!(err.to_string().contains("api_version") || err.to_string().contains("apiVersion"));
    }

    #[test]
    fn lint_folder_empty_directory_errors() {
        let dir = tempfile::tempdir().unwrap();
        let err = lint(dir.path()).unwrap_err();
        assert!(
            err.to_string().contains("no .wh files found"),
            "got: {}",
            err
        );
    }

    #[test]
    fn lint_folder_source_map_populated() {
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(
            dir.path().join("agents.wh"),
            "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: donna\n    image: d:latest\n",
        )
        .unwrap();

        std::fs::write(
            dir.path().join("streams.wh"),
            "api_version: wheelhouse.dev/v1\nname: dev\nstreams:\n  - name: main\n",
        )
        .unwrap();

        let linted = lint(dir.path()).unwrap();
        assert_eq!(
            linted.source_map().source_file("agent:donna"),
            Some("agents.wh")
        );
        assert_eq!(
            linted.source_map().source_file("stream:main"),
            Some("streams.wh")
        );
    }

    #[test]
    fn lint_single_file_backward_compatible() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(
            tmp,
            "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\nstreams:\n  - name: main\n"
        )
        .unwrap();

        let linted = lint(tmp.path()).unwrap();
        assert_eq!(linted.topology().name, "dev");
        assert_eq!(linted.topology().agents.len(), 1);
    }

    // ── Volume mount validation tests (ADR-043, ADR-044) ──

    #[test]
    fn validate_volume_mounts_passes_for_normal_subsystem() {
        use crate::deploy::{Agent, VolumeMount};

        let topology = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "lab".to_string(),
            broker: None,
            skills_repo: None,
            agents: vec![
                Agent {
                    name: "librarian-research".to_string(),
                    image: "librarian:latest".to_string(),
                    replicas: 1,
                    streams: vec![],
                    persona: None,
                    skills: None,
                    topology_edit: None,
                    volumes: vec![VolumeMount {
                        name: "wh-lab-llm-wiki-research".to_string(),
                        mount: "/workspace/.library".to_string(),
                        mount_mode: Some("rw".to_string()),
                    }],
                    env: None,
                },
                Agent {
                    name: "agent-a".to_string(),
                    image: "agent:latest".to_string(),
                    replicas: 1,
                    streams: vec![],
                    persona: None,
                    skills: None,
                    topology_edit: None,
                    volumes: vec![VolumeMount {
                        name: "wh-lab-llm-wiki-research".to_string(),
                        mount: "/workspace/.library".to_string(),
                        mount_mode: Some("ro".to_string()),
                    }],
                    env: None,
                },
            ],
            streams: vec![],
            surfaces: vec![],
            guardrails: None,
            subsystems: vec![],
        };

        let result = validate_volume_mounts(&topology);
        assert!(result.is_ok(), "expected ok, got: {result:?}");
    }

    #[test]
    fn validate_volume_mounts_rejects_collision_with_standard_volume() {
        use crate::deploy::{Agent, VolumeMount};

        let topology = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "lab".to_string(),
            broker: None,
            skills_repo: None,
            agents: vec![Agent {
                name: "librarian".to_string(),
                image: "librarian:latest".to_string(),
                replicas: 1,
                streams: vec![],
                persona: None,
                skills: None,
                topology_edit: None,
                volumes: vec![VolumeMount {
                    name: "wh-lab-wal".to_string(), // Collides with standard WAL volume
                    mount: "/data".to_string(),
                    mount_mode: Some("rw".to_string()),
                }],
                env: None,
            }],
            streams: vec![],
            surfaces: vec![],
            guardrails: None,
            subsystems: vec![],
        };

        let err = validate_volume_mounts(&topology).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Duplicate volume name") && msg.contains("wh-lab-wal"),
            "expected duplicate volume error, got: {msg}"
        );
    }

    #[test]
    fn validate_volume_mounts_rejects_multiple_rw_mounts() {
        use crate::deploy::{Agent, VolumeMount};

        let topology = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "lab".to_string(),
            broker: None,
            skills_repo: None,
            agents: vec![
                Agent {
                    name: "agent-a".to_string(),
                    image: "a:latest".to_string(),
                    replicas: 1,
                    streams: vec![],
                    persona: None,
                    skills: None,
                    topology_edit: None,
                    volumes: vec![VolumeMount {
                        name: "shared-vol".to_string(),
                        mount: "/data".to_string(),
                        mount_mode: Some("rw".to_string()),
                    }],
                    env: None,
                },
                Agent {
                    name: "agent-b".to_string(),
                    image: "b:latest".to_string(),
                    replicas: 1,
                    streams: vec![],
                    persona: None,
                    skills: None,
                    topology_edit: None,
                    volumes: vec![VolumeMount {
                        name: "shared-vol".to_string(),
                        mount: "/data".to_string(),
                        mount_mode: Some("rw".to_string()),
                    }],
                    env: None,
                },
            ],
            streams: vec![],
            surfaces: vec![],
            guardrails: None,
            subsystems: vec![],
        };

        let err = validate_volume_mounts(&topology).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("multiple rw mounts")
                && msg.contains("agent-a")
                && msg.contains("agent-b"),
            "expected single-writer violation, got: {msg}"
        );
    }

    #[test]
    fn validate_volume_mounts_default_mode_is_rw() {
        use crate::deploy::{Agent, VolumeMount};

        // When mount_mode is None, it defaults to "rw".
        // Two agents with no explicit mode on the same volume should trigger single-writer error.
        let topology = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "lab".to_string(),
            broker: None,
            skills_repo: None,
            agents: vec![
                Agent {
                    name: "agent-a".to_string(),
                    image: "a:latest".to_string(),
                    replicas: 1,
                    streams: vec![],
                    persona: None,
                    skills: None,
                    topology_edit: None,
                    volumes: vec![VolumeMount {
                        name: "shared-vol".to_string(),
                        mount: "/data".to_string(),
                        mount_mode: None, // defaults to rw
                    }],
                    env: None,
                },
                Agent {
                    name: "agent-b".to_string(),
                    image: "b:latest".to_string(),
                    replicas: 1,
                    streams: vec![],
                    persona: None,
                    skills: None,
                    topology_edit: None,
                    volumes: vec![VolumeMount {
                        name: "shared-vol".to_string(),
                        mount: "/data".to_string(),
                        mount_mode: None, // defaults to rw
                    }],
                    env: None,
                },
            ],
            streams: vec![],
            surfaces: vec![],
            guardrails: None,
            subsystems: vec![],
        };

        let err = validate_volume_mounts(&topology).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("multiple rw mounts"),
            "expected single-writer violation for default rw, got: {msg}"
        );
    }

    #[test]
    fn validate_volume_mounts_no_volumes_passes() {
        let topology = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "dev".to_string(),
            broker: None,
            skills_repo: None,
            agents: vec![crate::deploy::Agent {
                name: "researcher".to_string(),
                image: "r:latest".to_string(),
                replicas: 1,
                streams: vec![],
                persona: None,
                skills: None,
                topology_edit: None,
                volumes: vec![],
                env: None,
            }],
            streams: vec![],
            surfaces: vec![],
            guardrails: None,
            subsystems: vec![],
        };

        let result = validate_volume_mounts(&topology);
        assert!(result.is_ok());
    }
}
