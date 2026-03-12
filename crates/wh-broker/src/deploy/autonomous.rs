//! Autonomous observe-decide-act loop engine.
//!
//! Provides the core logic for an agent to evaluate stream signals,
//! propose topology changes, and apply them autonomously through
//! the deploy pipeline (lint -> plan_with_self_check -> commit -> apply).
//!
//! The agent runtime (Python SDK) drives this engine via CLI subprocess.
//! This module provides the Rust library interface.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::deploy::{
    parse_topology, Change, DeployError, Topology,
};
use crate::deploy::apply;
use crate::deploy::lint;
use crate::deploy::plan;

/// Pre-compiled regex for timeout signal pattern.
static TIMEOUT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(\d+)\s+daily\s+timeouts?\s+on\s+(\S+)").expect("invalid timeout regex")
});

/// Pre-compiled regex for high error rate signal pattern.
static ERROR_RATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)high\s+error\s+rate\s+on\s+(\S+)").expect("invalid error rate regex")
});

/// Confidence level for a proposed autonomous change.
///
/// Maps to human validation thresholds in Story 7.6.
/// In this story, all confidence levels proceed without human approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeConfidence {
    /// High confidence — routine scaling based on clear signal.
    High,
    /// Medium confidence — pattern detected but ambiguous.
    Medium,
    /// Low confidence — weak signal, may need human review.
    Low,
}

/// A proposed topology change from signal evaluation.
#[derive(Debug, Clone, PartialEq)]
pub enum ProposedChange {
    /// Scale an agent's replica count.
    ScaleAgent {
        agent_name: String,
        from_replicas: u32,
        to_replicas: u32,
    },
}

/// The result of evaluating a stream signal against the current topology.
#[derive(Debug, Clone)]
pub struct SignalEvaluation {
    /// Summary of the signal that triggered the evaluation.
    pub signal_summary: String,
    /// The proposed topology change.
    pub proposed_change: ProposedChange,
    /// Human-readable justification for the change.
    pub justification: String,
    /// Confidence level of the proposed change.
    pub confidence: ChangeConfidence,
}

/// The result of a successful autonomous apply.
#[derive(Debug, Clone)]
pub struct AutonomousApplyResult {
    /// The git commit summary line.
    pub commit_summary: String,
    /// The plan hash from the deploy pipeline.
    pub plan_hash: String,
    /// The changes that were applied.
    pub changes: Vec<Change>,
}

/// Notification data for surface publishing after an autonomous apply.
#[derive(Debug, Clone)]
pub struct AutonomousNotification {
    /// What changed in the topology.
    pub what_changed: String,
    /// Why the change was made (justification).
    pub why: String,
    /// Git commit reference (plan hash).
    pub commit_ref: String,
}

/// Evaluate a signal string against the current topology to propose a change.
///
/// Returns `None` if the signal is not recognized or the proposed change
/// would violate a guardrail.
///
/// MVP recognized patterns:
/// - `N daily timeouts on AGENT_NAME` where N >= 3 proposes scale-up by 1
/// - `high error rate on AGENT_NAME` proposes scale-up by 1
#[tracing::instrument(skip_all, fields(signal = %signal))]
pub fn evaluate_signal(signal: &str, current_topology: &Topology) -> Option<SignalEvaluation> {
    // Pattern 1: "N daily timeouts on AGENT_NAME" where N >= 3
    if let Some(caps) = TIMEOUT_RE.captures(signal) {
        let count: u32 = caps[1].parse().ok()?;
        let agent_name = caps[2].to_string();

        if count < 3 {
            return None;
        }

        return propose_scale_up(&agent_name, current_topology, signal, &format!(
            "Detected {} daily timeouts on agent '{}', indicating insufficient capacity. Scaling up by 1 replica.",
            count, agent_name
        ));
    }

    // Pattern 2: "high error rate on AGENT_NAME"
    if let Some(caps) = ERROR_RATE_RE.captures(signal) {
        let agent_name = caps[1].to_string();

        return propose_scale_up(&agent_name, current_topology, signal, &format!(
            "High error rate detected on agent '{}', suggesting resource exhaustion. Scaling up by 1 replica.",
            agent_name
        ));
    }

    None
}

/// Propose scaling up an agent by 1 replica, respecting guardrails.
fn propose_scale_up(
    agent_name: &str,
    topology: &Topology,
    signal: &str,
    justification: &str,
) -> Option<SignalEvaluation> {
    // Find the agent in the current topology
    let agent = topology.agents.iter().find(|a| a.name == agent_name)?;
    let new_replicas = agent.replicas + 1;

    // Pre-validate against guardrails
    if let Some(ref guardrails) = topology.guardrails {
        if let Some(max) = guardrails.max_replicas {
            if new_replicas > max {
                return None;
            }
        }
    }

    Some(SignalEvaluation {
        signal_summary: signal.to_string(),
        proposed_change: ProposedChange::ScaleAgent {
            agent_name: agent_name.to_string(),
            from_replicas: agent.replicas,
            to_replicas: new_replicas,
        },
        justification: justification.to_string(),
        confidence: ChangeConfidence::High,
    })
}

/// Modify a `.wh` topology file content to update an agent's replica count.
///
/// Parses the YAML, updates the target agent's replicas, and re-serializes.
/// Returns the modified YAML string.
///
/// Note: serde_yaml round-trip may lose comments and reorder keys. Acceptable for MVP.
#[tracing::instrument(skip_all, fields(agent_name = %agent_name, new_replicas = %new_replicas))]
pub fn modify_topology_replicas(
    content: &str,
    agent_name: &str,
    new_replicas: u32,
) -> Result<String, DeployError> {
    let mut topology: Topology = parse_topology(content)?;

    let agent = topology
        .agents
        .iter_mut()
        .find(|a| a.name == agent_name)
        .ok_or_else(|| {
            DeployError::InvalidTopology(format!(
                "agent '{}' not found in topology",
                agent_name
            ))
        })?;

    agent.replicas = new_replicas;

    serde_yaml::to_string(&topology).map_err(|e| {
        DeployError::InvalidTopology(format!("failed to serialize modified topology: {e}"))
    })
}

/// Apply an autonomous topology change through the full deploy pipeline.
///
/// Reads the `.wh` file, modifies it based on the evaluation, writes it back,
/// then runs the full pipeline: lint -> plan_with_self_check -> commit -> apply.
///
/// The commit message includes the agent's justification (ADR-003 extension).
#[tracing::instrument(skip_all, fields(agent_name = %agent_name))]
pub fn apply_autonomous_change(
    evaluation: SignalEvaluation,
    wh_path: &Path,
    agent_name: &str,
) -> Result<AutonomousApplyResult, DeployError> {
    // Extract change details
    let (target_agent, new_replicas) = match &evaluation.proposed_change {
        ProposedChange::ScaleAgent {
            agent_name,
            to_replicas,
            ..
        } => (agent_name.clone(), *to_replicas),
    };

    // Read current .wh file (saved for rollback on failure)
    let original_content = std::fs::read_to_string(wh_path).map_err(DeployError::FileRead)?;

    // Modify the topology
    let modified = modify_topology_replicas(&original_content, &target_agent, new_replicas)?;

    // Write the modified .wh file back
    std::fs::write(wh_path, &modified).map_err(DeployError::FileRead)?;

    // Run the full deploy pipeline: lint -> plan_with_self_check -> commit -> apply.
    // On any failure, restore the original .wh file to avoid corrupting the working directory.
    let pipeline_result = (|| -> Result<(String, Vec<Change>), DeployError> {
        let linted = lint::lint(wh_path)?;
        let plan_output = plan::plan_with_self_check(linted, Some(agent_name))?;

        let plan_hash = plan_output.plan_hash().to_string();
        let changes = plan_output.changes().to_vec();

        // Commit with justification in the message body
        let committed = apply::commit(plan_output, Some(agent_name))?;

        // Apply the committed plan
        let _apply_result = apply::apply(committed)?;

        Ok((plan_hash, changes))
    })();

    let (plan_hash, changes) = match pipeline_result {
        Ok(result) => result,
        Err(e) => {
            // Rollback: restore the original .wh file content
            let _ = std::fs::write(wh_path, &original_content);
            return Err(e);
        }
    };

    let commit_summary = format!(
        "[{}] apply: scale agent {} replicas to {}",
        agent_name, target_agent, new_replicas
    );

    Ok(AutonomousApplyResult {
        commit_summary,
        plan_hash,
        changes,
    })
}

/// Format a notification for surface publishing after an autonomous apply.
///
/// Returns structured data that the agent can publish via its configured surface.
pub fn format_notification(
    result: &AutonomousApplyResult,
    evaluation: &SignalEvaluation,
) -> AutonomousNotification {
    let what_changed = match &evaluation.proposed_change {
        ProposedChange::ScaleAgent {
            agent_name,
            from_replicas,
            to_replicas,
        } => format!(
            "Scaled agent '{}' from {} to {} replicas",
            agent_name, from_replicas, to_replicas
        ),
    };

    AutonomousNotification {
        what_changed,
        why: evaluation.justification.clone(),
        commit_ref: result.plan_hash.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy::{Agent, Guardrails};

    fn make_topology(agents: Vec<Agent>, guardrails: Option<Guardrails>) -> Topology {
        Topology {
            api_version: "wheelhouse.dev/v1".to_string(),
            name: "dev".to_string(),
            agents,
            streams: vec![],
            guardrails,
        }
    }

    fn make_agent(name: &str, replicas: u32) -> Agent {
        Agent {
            name: name.to_string(),
            image: format!("{name}:latest"),
            replicas,
            streams: vec![],
        }
    }

    // =========================================================================
    // Signal evaluation tests
    // =========================================================================

    #[test]
    fn evaluate_timeout_signal_proposes_scale_up() {
        let topology = make_topology(vec![make_agent("researcher", 1)], None);
        let eval = evaluate_signal("4 daily timeouts on researcher", &topology);
        assert!(eval.is_some());
        let eval = eval.unwrap();
        assert_eq!(
            eval.proposed_change,
            ProposedChange::ScaleAgent {
                agent_name: "researcher".to_string(),
                from_replicas: 1,
                to_replicas: 2,
            }
        );
        assert!(!eval.justification.is_empty());
        assert_eq!(eval.confidence, ChangeConfidence::High);
    }

    #[test]
    fn evaluate_low_timeout_count_returns_none() {
        let topology = make_topology(vec![make_agent("researcher", 1)], None);
        let eval = evaluate_signal("2 daily timeouts on researcher", &topology);
        assert!(eval.is_none());
    }

    #[test]
    fn evaluate_high_error_rate_proposes_scale_up() {
        let topology = make_topology(vec![make_agent("researcher", 1)], None);
        let eval = evaluate_signal("high error rate on researcher", &topology);
        assert!(eval.is_some());
        let eval = eval.unwrap();
        match eval.proposed_change {
            ProposedChange::ScaleAgent { to_replicas, .. } => assert_eq!(to_replicas, 2),
        }
    }

    #[test]
    fn evaluate_unrecognized_signal_returns_none() {
        let topology = make_topology(vec![make_agent("researcher", 1)], None);
        let eval = evaluate_signal("hello world nothing to see", &topology);
        assert!(eval.is_none());
    }

    #[test]
    fn evaluate_signal_respects_guardrail() {
        let topology = make_topology(
            vec![make_agent("researcher", 1)],
            Some(Guardrails { max_replicas: Some(1) }),
        );
        let eval = evaluate_signal("4 daily timeouts on researcher", &topology);
        assert!(eval.is_none());
    }

    #[test]
    fn evaluate_signal_nonexistent_agent_returns_none() {
        let topology = make_topology(vec![make_agent("donna", 1)], None);
        let eval = evaluate_signal("4 daily timeouts on researcher", &topology);
        assert!(eval.is_none());
    }

    #[test]
    fn evaluate_signal_case_insensitive() {
        let topology = make_topology(vec![make_agent("researcher", 1)], None);
        let eval = evaluate_signal("4 Daily Timeouts on researcher", &topology);
        assert!(eval.is_some());
    }

    // =========================================================================
    // Topology modification tests
    // =========================================================================

    #[test]
    fn modify_replicas_updates_target_agent() {
        let content = "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\n    replicas: 1\n";
        let modified = modify_topology_replicas(content, "researcher", 2).unwrap();
        let topo = parse_topology(&modified).unwrap();
        assert_eq!(topo.agents[0].replicas, 2);
    }

    #[test]
    fn modify_replicas_errors_on_nonexistent_agent() {
        let content = "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\n";
        let result = modify_topology_replicas(content, "nonexistent", 2);
        assert!(result.is_err());
    }

    #[test]
    fn modify_replicas_preserves_other_agents() {
        let content = "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\n    replicas: 1\n  - name: donna\n    image: donna:latest\n    replicas: 1\nstreams:\n  - name: main\n";
        let modified = modify_topology_replicas(content, "researcher", 3).unwrap();
        let topo = parse_topology(&modified).unwrap();
        let donna = topo.agents.iter().find(|a| a.name == "donna").unwrap();
        assert_eq!(donna.replicas, 1);
        assert_eq!(topo.streams.len(), 1);
    }

    // =========================================================================
    // Notification tests
    // =========================================================================

    #[test]
    fn notification_contains_required_fields() {
        let result = AutonomousApplyResult {
            commit_summary: "[donna] apply: scale agent researcher replicas to 2".to_string(),
            plan_hash: "sha256:abc123".to_string(),
            changes: vec![],
        };
        let evaluation = SignalEvaluation {
            signal_summary: "4 daily timeouts on researcher".to_string(),
            proposed_change: ProposedChange::ScaleAgent {
                agent_name: "researcher".to_string(),
                from_replicas: 1,
                to_replicas: 2,
            },
            justification: "Recurring timeouts suggest insufficient capacity".to_string(),
            confidence: ChangeConfidence::High,
        };
        let notification = format_notification(&result, &evaluation);
        assert!(!notification.what_changed.is_empty());
        assert!(!notification.why.is_empty());
        assert!(!notification.commit_ref.is_empty());
        assert!(notification.what_changed.contains("researcher"));
        assert_eq!(notification.commit_ref, "sha256:abc123");
    }
}
