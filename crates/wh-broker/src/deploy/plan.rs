//! Plan step of the deploy pipeline.
//!
//! Consumes a `LintedFile` and produces a `PlanOutput` typestate token.
//! The plan is a pure in-memory diff with no side effects (ADR-003).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::deploy::{
    canonicalize_topology, Agent, Change, DeployError, Stream, Topology,
};
use crate::deploy::lint::LintedFile;

/// The output of a deploy plan operation.
///
/// Contains the diff between current and desired topology state.
/// Must be consumed by `commit()` in the apply step.
#[must_use = "a PlanOutput must be passed to commit() — do not discard"]
#[derive(Debug)]
pub struct PlanOutput {
    pub(crate) has_changes: bool,
    pub(crate) changes: Vec<Change>,
    pub(crate) plan_hash: String,
    pub(crate) topology_name: String,
    pub(crate) policy_snapshot_hash: String,
    pub(crate) warnings: Vec<String>,
    pub(crate) desired_topology: Topology,
    pub(crate) source_path: PathBuf,
}

impl PlanOutput {
    pub fn has_changes(&self) -> bool {
        self.has_changes
    }

    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    pub fn plan_hash(&self) -> &str {
        &self.plan_hash
    }

    pub fn topology_name(&self) -> &str {
        &self.topology_name
    }

    pub fn policy_snapshot_hash(&self) -> &str {
        &self.policy_snapshot_hash
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn desired_topology(&self) -> &Topology {
        &self.desired_topology
    }

    pub fn source_path(&self) -> &Path {
        &self.source_path
    }
}

/// Serializable plan data for JSON output.
/// Matches the architecture-specified response schema.
#[derive(Debug, Serialize, Deserialize)]
pub struct PlanData {
    pub has_changes: bool,
    pub changes: Vec<Change>,
    pub plan_hash: String,
    pub topology_name: String,
    pub policy_snapshot_hash: String,
    pub warnings: Vec<String>,
}

impl std::fmt::Display for PlanData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.has_changes {
            writeln!(f, "No changes. Infrastructure is up-to-date.")?;
            return Ok(());
        }

        writeln!(f, "Changes detected in topology '{}':", self.topology_name)?;
        writeln!(f)?;

        let mut create_count: usize = 0;
        let mut update_count: usize = 0;
        let mut destroy_count: usize = 0;

        for change in &self.changes {
            match change.op.as_str() {
                "+" => {
                    create_count += 1;
                    // Show provider info for streams if available
                    if let Some(to) = &change.to {
                        if let Some(provider) = to.get("provider").and_then(|v| v.as_str()) {
                            writeln!(f, "  + {} (new, provider: {})", change.component, provider)?;
                        } else {
                            writeln!(f, "  + {} (new)", change.component)?;
                        }
                    } else {
                        writeln!(f, "  + {} (new)", change.component)?;
                    }
                }
                "-" => {
                    destroy_count += 1;
                    writeln!(f, "  - {} (destroy)", change.component)?;
                }
                "~" => {
                    update_count += 1;
                    if let (Some(field), Some(from), Some(to)) =
                        (&change.field, &change.from, &change.to)
                    {
                        writeln!(f, "  ~ {} {}: {} -> {}", change.component, field, from, to)?;
                    } else {
                        writeln!(f, "  ~ {}", change.component)?;
                    }
                }
                _ => writeln!(f, "  {} {}", change.op, change.component)?,
            }
        }
        writeln!(f)?;
        writeln!(
            f,
            "{} to create \u{00B7} {} to update \u{00B7} {} to destroy",
            create_count, update_count, destroy_count
        )?;
        writeln!(f)?;
        writeln!(f, "Plan hash: {}", self.plan_hash)?;

        for warning in &self.warnings {
            writeln!(f, "Warning: {}", warning)?;
        }

        Ok(())
    }
}

impl From<&PlanOutput> for PlanData {
    fn from(plan: &PlanOutput) -> Self {
        PlanData {
            has_changes: plan.has_changes,
            changes: plan.changes.clone(),
            plan_hash: plan.plan_hash.clone(),
            topology_name: plan.topology_name.clone(),
            policy_snapshot_hash: plan.policy_snapshot_hash.clone(),
            warnings: plan.warnings.clone(),
        }
    }
}

/// Load the current applied state from `.wh/state.json`.
/// Returns `None` if no state file exists (first deploy).
fn load_current_state(workspace_root: &Path) -> Result<Option<Topology>, DeployError> {
    let state_path = workspace_root.join(".wh").join("state.json");
    if !state_path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&state_path).map_err(DeployError::FileRead)?;
    let topology: Topology = serde_json::from_str(&content)
        .map_err(|e| DeployError::PlanFailed(format!("corrupt state file: {e}")))?;
    Ok(Some(topology))
}

/// Compute the diff between current and desired topology.
fn diff_topologies(current: &Topology, desired: &Topology) -> Vec<Change> {
    let mut changes = Vec::new();

    // Diff agents
    let current_agents: std::collections::BTreeMap<&str, &Agent> =
        current.agents.iter().map(|a| (a.name.as_str(), a)).collect();
    let desired_agents: std::collections::BTreeMap<&str, &Agent> =
        desired.agents.iter().map(|a| (a.name.as_str(), a)).collect();

    // Added agents
    for (name, agent) in &desired_agents {
        if !current_agents.contains_key(name) {
            changes.push(Change {
                op: "+".to_string(),
                component: format!("agent {name}"),
                field: None,
                from: None,
                to: Some(serde_json::json!({
                    "image": agent.image,
                    "replicas": agent.replicas,
                })),
            });
        }
    }

    // Removed agents
    for name in current_agents.keys() {
        if !desired_agents.contains_key(name) {
            changes.push(Change {
                op: "-".to_string(),
                component: format!("agent {name}"),
                field: None,
                from: None,
                to: None,
            });
        }
    }

    // Modified agents
    for (name, desired_agent) in &desired_agents {
        if let Some(current_agent) = current_agents.get(name) {
            if current_agent.replicas != desired_agent.replicas {
                changes.push(Change {
                    op: "~".to_string(),
                    component: format!("agent {name}"),
                    field: Some("replicas".to_string()),
                    from: Some(serde_json::json!(current_agent.replicas)),
                    to: Some(serde_json::json!(desired_agent.replicas)),
                });
            }
            if current_agent.image != desired_agent.image {
                changes.push(Change {
                    op: "~".to_string(),
                    component: format!("agent {name}"),
                    field: Some("image".to_string()),
                    from: Some(serde_json::json!(current_agent.image)),
                    to: Some(serde_json::json!(desired_agent.image)),
                });
            }
        }
    }

    // Diff streams
    let current_streams: std::collections::BTreeMap<&str, &Stream> =
        current.streams.iter().map(|s| (s.name.as_str(), s)).collect();
    let desired_streams: std::collections::BTreeMap<&str, &Stream> =
        desired.streams.iter().map(|s| (s.name.as_str(), s)).collect();

    for name in desired_streams.keys() {
        if !current_streams.contains_key(name) {
            // All streams use local provider in MVP
            let provider = "local";
            changes.push(Change {
                op: "+".to_string(),
                component: format!("stream {name}"),
                field: None,
                from: None,
                to: Some(serde_json::json!({
                    "provider": provider,
                })),
            });
        }
    }

    for name in current_streams.keys() {
        if !desired_streams.contains_key(name) {
            changes.push(Change {
                op: "-".to_string(),
                component: format!("stream {name}"),
                field: None,
                from: None,
                to: None,
            });
        }
    }

    for (name, desired_stream) in &desired_streams {
        if let Some(current_stream) = current_streams.get(name) {
            if current_stream.retention != desired_stream.retention {
                changes.push(Change {
                    op: "~".to_string(),
                    component: format!("stream {name}"),
                    field: Some("retention".to_string()),
                    from: Some(serde_json::json!(current_stream.retention)),
                    to: Some(serde_json::json!(desired_stream.retention)),
                });
            }
        }
    }

    changes
}

/// Compute the canonical plan hash (SHA-256 over sorted, whitespace-normalized JSON).
///
/// The hash is computed over the canonical JSON representation of the changes
/// array, ensuring deterministic output regardless of input ordering.
fn compute_plan_hash(changes: &[Change]) -> String {
    // Canonical serialization: serde_json with sorted keys produces deterministic output
    // We sort the changes by component name and operation for canonical ordering
    let mut sorted_changes = changes.to_vec();
    sorted_changes.sort_by(|a, b| {
        a.component
            .cmp(&b.component)
            .then(a.op.cmp(&b.op))
            .then(a.field.cmp(&b.field))
    });

    let canonical_json =
        serde_json::to_string(&sorted_changes).unwrap_or_else(|_| "[]".to_string());

    let mut hasher = Sha256::new();
    hasher.update(canonical_json.as_bytes());
    let result = hasher.finalize();
    format!("sha256:{:x}", result)
}

/// Execute the plan step: consume a `LintedFile` and produce a `PlanOutput`.
///
/// The plan is a pure in-memory diff with no side effects (ADR-003).
/// Compares the desired state (from the linted file) against the current
/// applied state (from `.wh/state.json`, or empty if first deploy).
///
/// If `force_destroy_all` is false and the plan would destroy all agents
/// from a non-empty current state, returns `DeployError::PolicyViolation` (CM-05).
#[tracing::instrument(skip_all, fields(topology = %linted.topology().name))]
pub fn plan(linted: LintedFile) -> Result<PlanOutput, DeployError> {
    plan_with_options(linted, false)
}

/// Execute the plan step with options.
///
/// `force_destroy_all`: if true, allows plans that would destroy all agents (CM-05).
#[tracing::instrument(skip_all, fields(topology = %linted.topology().name))]
pub fn plan_with_options(linted: LintedFile, force_destroy_all: bool) -> Result<PlanOutput, DeployError> {
    let desired = canonicalize_topology(linted.topology.clone());
    let workspace_root = linted
        .source_path
        .parent()
        .unwrap_or_else(|| Path::new("."));

    let current = load_current_state(workspace_root)?;

    let changes = match &current {
        Some(current_topo) => {
            let canonical_current = canonicalize_topology(current_topo.clone());
            diff_topologies(&canonical_current, &desired)
        }
        None => {
            // First deploy: everything is an addition
            let empty = Topology {
                api_version: "wheelhouse.dev/v1".to_string(),
                name: desired.name.clone(),
                agents: vec![],
                streams: vec![],
                guardrails: None,
            };
            diff_topologies(&empty, &desired)
        }
    };

    let has_changes = !changes.is_empty();
    let plan_hash = compute_plan_hash(&changes);
    let topology_name = desired.name.clone();

    // Guardrail validation (RT-04): check max_replicas constraint
    if let Some(ref guardrails) = desired.guardrails {
        if let Some(max_replicas) = guardrails.max_replicas {
            for agent in &desired.agents {
                if agent.replicas > max_replicas {
                    return Err(DeployError::PolicyViolation(format!(
                        "agent '{}' requests {} replicas, exceeds max_replicas {}",
                        agent.name, agent.replicas, max_replicas
                    )));
                }
            }
        }
    }

    // Self-destruct detection (CM-05): block plans that would destroy all agents
    // from a non-empty current state, unless --force-destroy-all is provided.
    let current_has_agents = current
        .as_ref()
        .map(|t| !t.agents.is_empty())
        .unwrap_or(false);
    let desired_has_no_agents = desired.agents.is_empty();
    let is_self_destruct = current_has_agents && desired_has_no_agents;

    if is_self_destruct && !force_destroy_all {
        return Err(DeployError::PolicyViolation(
            "plan would destroy all agents (self-destruct detected). \
             Use --force-destroy-all to proceed."
                .to_string(),
        ));
    }

    // No policy system yet — return empty hash
    let policy_snapshot_hash = String::new();

    let mut warnings = Vec::new();

    if is_self_destruct && force_destroy_all {
        warnings.push(
            "all agents will be destroyed — self-destruct plan approved with --force-destroy-all"
                .to_string(),
        );
    }

    Ok(PlanOutput {
        has_changes,
        changes,
        plan_hash,
        topology_name,
        policy_snapshot_hash,
        warnings,
        desired_topology: desired,
        source_path: linted.source_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn create_temp_wh(content: &str) -> tempfile::NamedTempFile {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        write!(tmp, "{}", content).unwrap();
        tmp
    }

    #[test]
    fn plan_first_deploy_shows_all_additions() {
        let tmp = create_temp_wh(
            "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\nstreams:\n  - name: main\n",
        );
        let linted = crate::deploy::lint::lint(tmp.path()).unwrap();
        let plan_output = plan(linted).unwrap();

        assert!(plan_output.has_changes());
        assert!(!plan_output.changes().is_empty());
        assert!(plan_output.plan_hash().starts_with("sha256:"));
        assert_eq!(plan_output.topology_name(), "dev");
    }

    #[test]
    fn plan_no_changes_when_state_matches() {
        let dir = tempfile::tempdir().unwrap();
        let wh_dir = dir.path().join(".wh");
        std::fs::create_dir_all(&wh_dir).unwrap();

        let topology = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "dev".to_string(),
            agents: vec![Agent {
                name: "researcher".to_string(),
                image: "r:latest".to_string(),
                replicas: 1,
                streams: vec!["main".to_string()],
            }],
            streams: vec![Stream {
                name: "main".to_string(),
                retention: Some("7d".to_string()),
            }],
            guardrails: None,
        };
        std::fs::write(
            wh_dir.join("state.json"),
            serde_json::to_string(&topology).unwrap(),
        )
        .unwrap();

        let wh_path = dir.path().join("topology.wh");
        std::fs::write(
            &wh_path,
            "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\n    replicas: 1\n    streams:\n      - main\nstreams:\n  - name: main\n    retention: 7d\n",
        )
        .unwrap();

        let linted = crate::deploy::lint::lint(&wh_path).unwrap();
        let plan_output = plan(linted).unwrap();

        assert!(!plan_output.has_changes());
        assert!(plan_output.changes().is_empty());
    }

    #[test]
    fn plan_detects_replica_change() {
        let dir = tempfile::tempdir().unwrap();
        let wh_dir = dir.path().join(".wh");
        std::fs::create_dir_all(&wh_dir).unwrap();

        let topology = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "dev".to_string(),
            agents: vec![Agent {
                name: "researcher".to_string(),
                image: "r:latest".to_string(),
                replicas: 1,
                streams: vec![],
            }],
            streams: vec![],
            guardrails: None,
        };
        std::fs::write(
            wh_dir.join("state.json"),
            serde_json::to_string(&topology).unwrap(),
        )
        .unwrap();

        let wh_path = dir.path().join("topology.wh");
        std::fs::write(
            &wh_path,
            "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\n    replicas: 2\n",
        )
        .unwrap();

        let linted = crate::deploy::lint::lint(&wh_path).unwrap();
        let plan_output = plan(linted).unwrap();

        assert!(plan_output.has_changes());
        assert_eq!(plan_output.changes().len(), 1);
        assert_eq!(plan_output.changes()[0].op, "~");
        assert_eq!(plan_output.changes()[0].component, "agent researcher");
        assert_eq!(
            plan_output.changes()[0].field.as_deref(),
            Some("replicas")
        );
    }

    #[test]
    fn plan_hash_is_deterministic() {
        let tmp = create_temp_wh(
            "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\n",
        );
        let linted1 = crate::deploy::lint::lint(tmp.path()).unwrap();
        let plan1 = plan(linted1).unwrap();

        let linted2 = crate::deploy::lint::lint(tmp.path()).unwrap();
        let plan2 = plan(linted2).unwrap();

        assert_eq!(plan1.plan_hash(), plan2.plan_hash());
    }

    #[test]
    fn plan_hash_canonical_ordering() {
        // Two files with same content but different YAML key order should produce same hash
        let dir = tempfile::tempdir().unwrap();

        let wh1 = dir.path().join("a.wh");
        std::fs::write(
            &wh1,
            "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: alpha\n    image: a:latest\n  - name: beta\n    image: b:latest\n",
        )
        .unwrap();

        let wh2 = dir.path().join("b.wh");
        std::fs::write(
            &wh2,
            "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: beta\n    image: b:latest\n  - name: alpha\n    image: a:latest\n",
        )
        .unwrap();

        let linted1 = crate::deploy::lint::lint(&wh1).unwrap();
        let plan1 = plan(linted1).unwrap();

        let linted2 = crate::deploy::lint::lint(&wh2).unwrap();
        let plan2 = plan(linted2).unwrap();

        assert_eq!(plan1.plan_hash(), plan2.plan_hash());
    }

    #[test]
    fn plan_detects_agent_addition() {
        let dir = tempfile::tempdir().unwrap();
        let wh_dir = dir.path().join(".wh");
        std::fs::create_dir_all(&wh_dir).unwrap();

        let topology = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "dev".to_string(),
            agents: vec![],
            streams: vec![],
            guardrails: None,
        };
        std::fs::write(
            wh_dir.join("state.json"),
            serde_json::to_string(&topology).unwrap(),
        )
        .unwrap();

        let wh_path = dir.path().join("topology.wh");
        std::fs::write(
            &wh_path,
            "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\n",
        )
        .unwrap();

        let linted = crate::deploy::lint::lint(&wh_path).unwrap();
        let plan_output = plan(linted).unwrap();

        assert!(plan_output.has_changes());
        assert_eq!(plan_output.changes()[0].op, "+");
    }

    #[test]
    fn plan_detects_agent_removal() {
        let dir = tempfile::tempdir().unwrap();
        let wh_dir = dir.path().join(".wh");
        std::fs::create_dir_all(&wh_dir).unwrap();

        let topology = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "dev".to_string(),
            agents: vec![Agent {
                name: "researcher".to_string(),
                image: "r:latest".to_string(),
                replicas: 1,
                streams: vec![],
            }],
            streams: vec![],
            guardrails: None,
        };
        std::fs::write(
            wh_dir.join("state.json"),
            serde_json::to_string(&topology).unwrap(),
        )
        .unwrap();

        let wh_path = dir.path().join("topology.wh");
        std::fs::write(
            &wh_path,
            "api_version: wheelhouse.dev/v1\nname: dev\nagents: []\n",
        )
        .unwrap();

        // This removes all agents (self-destruct), so --force-destroy-all is needed (CM-05)
        let linted = crate::deploy::lint::lint(&wh_path).unwrap();
        let plan_output = plan_with_options(linted, true).unwrap();

        assert!(plan_output.has_changes());
        assert_eq!(plan_output.changes()[0].op, "-");
    }

    #[test]
    fn plan_blocks_self_destruct_without_force_flag() {
        let dir = tempfile::tempdir().unwrap();
        let wh_dir = dir.path().join(".wh");
        std::fs::create_dir_all(&wh_dir).unwrap();

        let topology = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "dev".to_string(),
            agents: vec![Agent {
                name: "researcher".to_string(),
                image: "r:latest".to_string(),
                replicas: 1,
                streams: vec![],
            }],
            streams: vec![],
            guardrails: None,
        };
        std::fs::write(
            wh_dir.join("state.json"),
            serde_json::to_string(&topology).unwrap(),
        )
        .unwrap();

        let wh_path = dir.path().join("topology.wh");
        std::fs::write(
            &wh_path,
            "api_version: wheelhouse.dev/v1\nname: dev\nagents: []\n",
        )
        .unwrap();

        let linted = crate::deploy::lint::lint(&wh_path).unwrap();
        let err = plan(linted).unwrap_err();
        assert!(matches!(err, DeployError::PolicyViolation(_)));
        let msg = err.to_string();
        assert!(msg.contains("self-destruct"), "error should mention self-destruct: {msg}");
        assert!(msg.contains("--force-destroy-all"), "error should mention --force-destroy-all: {msg}");
    }

    #[test]
    fn plan_self_destruct_with_force_includes_warning() {
        let dir = tempfile::tempdir().unwrap();
        let wh_dir = dir.path().join(".wh");
        std::fs::create_dir_all(&wh_dir).unwrap();

        let topology = Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "dev".to_string(),
            agents: vec![Agent {
                name: "researcher".to_string(),
                image: "r:latest".to_string(),
                replicas: 1,
                streams: vec![],
            }],
            streams: vec![],
            guardrails: None,
        };
        std::fs::write(
            wh_dir.join("state.json"),
            serde_json::to_string(&topology).unwrap(),
        )
        .unwrap();

        let wh_path = dir.path().join("topology.wh");
        std::fs::write(
            &wh_path,
            "api_version: wheelhouse.dev/v1\nname: dev\nagents: []\n",
        )
        .unwrap();

        let linted = crate::deploy::lint::lint(&wh_path).unwrap();
        let plan_output = plan_with_options(linted, true).unwrap();

        assert!(plan_output.has_changes());
        assert!(!plan_output.warnings().is_empty(), "should have self-destruct warning");
        assert!(
            plan_output.warnings()[0].contains("destroy"),
            "warning should mention destroy: {}",
            plan_output.warnings()[0]
        );
    }

    #[test]
    fn plan_data_display_shows_new_annotations_and_summary() {
        let plan_data = PlanData {
            has_changes: true,
            changes: vec![
                Change {
                    op: "+".to_string(),
                    component: "agent researcher".to_string(),
                    field: None,
                    from: None,
                    to: Some(serde_json::json!({"image": "r:latest", "replicas": 1})),
                },
                Change {
                    op: "+".to_string(),
                    component: "stream main".to_string(),
                    field: None,
                    from: None,
                    to: Some(serde_json::json!({"provider": "local"})),
                },
            ],
            plan_hash: "sha256:abc123".to_string(),
            topology_name: "dev".to_string(),
            policy_snapshot_hash: String::new(),
            warnings: vec![],
        };

        let output = format!("{plan_data}");
        assert!(output.contains("+ agent researcher (new)"), "should show (new): {output}");
        assert!(output.contains("+ stream main (new, provider: local)"), "should show stream with provider: {output}");
        assert!(output.contains("2 to create"), "should show create count: {output}");
        assert!(output.contains("0 to update"), "should show update count: {output}");
        assert!(output.contains("0 to destroy"), "should show destroy count: {output}");
    }

    #[test]
    fn plan_data_display_shows_no_changes_message() {
        let plan_data = PlanData {
            has_changes: false,
            changes: vec![],
            plan_hash: "sha256:empty".to_string(),
            topology_name: "dev".to_string(),
            policy_snapshot_hash: String::new(),
            warnings: vec![],
        };

        let output = format!("{plan_data}");
        assert!(output.contains("No changes"), "should show no changes: {output}");
        assert!(output.contains("up-to-date"), "should show up-to-date: {output}");
    }

    #[test]
    fn plan_data_display_shows_destroy_annotation() {
        let plan_data = PlanData {
            has_changes: true,
            changes: vec![Change {
                op: "-".to_string(),
                component: "agent researcher".to_string(),
                field: None,
                from: None,
                to: None,
            }],
            plan_hash: "sha256:abc".to_string(),
            topology_name: "dev".to_string(),
            policy_snapshot_hash: String::new(),
            warnings: vec![],
        };

        let output = format!("{plan_data}");
        assert!(output.contains("- agent researcher (destroy)"), "should show (destroy): {output}");
        assert!(output.contains("1 to destroy"), "should show destroy count: {output}");
    }
}
