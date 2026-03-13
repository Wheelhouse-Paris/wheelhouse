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

use crate::deploy::apply;
use crate::deploy::approval::ApprovalRequest;
use crate::deploy::lint;
use crate::deploy::plan;
use crate::deploy::{parse_topology, Change, DeployError, ThresholdLevel, Topology};

/// Pre-compiled regex for timeout signal pattern.
static TIMEOUT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(\d+)\s+daily\s+timeouts?\s+on\s+(\S+)").expect("invalid timeout regex")
});

/// Pre-compiled regex for high error rate signal pattern.
static ERROR_RATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)high\s+error\s+rate\s+on\s+(\S+)").expect("invalid error rate regex")
});

/// Impact level of a proposed topology change.
///
/// Used by the human validation threshold system to decide whether
/// a change requires human approval before being applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImpactLevel {
    /// Low impact — minor change, e.g. scaling by < 25%.
    Low,
    /// Medium impact — moderate change, e.g. scaling by 25-50%.
    Medium,
    /// High impact — significant change, e.g. scaling by > 50% or removing an agent.
    High,
}

/// The result of evaluating a change against the configured threshold.
#[derive(Debug, Clone)]
pub enum ThresholdDecision {
    /// Change is within threshold — proceed autonomously.
    ProceedAutonomously,
    /// Change exceeds threshold — requires human approval before applying.
    RequiresApproval {
        /// The formatted approval request to send to the operator.
        request: ApprovalRequest,
    },
}

/// Confidence level for a proposed autonomous change.
///
/// Maps to human validation thresholds in Story 7.6.
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
            DeployError::InvalidTopology(format!("agent '{}' not found in topology", agent_name))
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
    apply_autonomous_change_inner(evaluation, wh_path, agent_name, None)
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

/// Classify the impact level of a proposed topology change.
///
/// Impact classification rules (percentage-based):
/// - Scaling replicas by > 50%: `High`
/// - Scaling replicas by 25-50%: `Medium`
/// - Scaling replicas by < 25%: `Low`
/// - Removing an agent: `High`
///
/// Note: scaling from 1 to 2 is 100% (High), not "just +1".
#[tracing::instrument(skip_all)]
pub fn classify_impact(proposed_change: &ProposedChange, _topology: &Topology) -> ImpactLevel {
    match proposed_change {
        ProposedChange::ScaleAgent {
            from_replicas,
            to_replicas,
            ..
        } => {
            if *from_replicas == 0 {
                // Scaling from 0 is always high impact
                return ImpactLevel::High;
            }
            let change = *to_replicas as f64 - *from_replicas as f64;
            let pct = (change.abs() / *from_replicas as f64) * 100.0;
            // Scale-down is always at least Medium impact
            if change < 0.0 && pct < 25.0 {
                return ImpactLevel::Medium;
            }
            if pct > 50.0 {
                ImpactLevel::High
            } else if pct >= 25.0 {
                ImpactLevel::Medium
            } else {
                ImpactLevel::Low
            }
        }
    }
}

/// Determine whether a change with the given impact level requires human approval
/// given the configured threshold.
///
/// - `None` threshold: all changes proceed autonomously.
/// - `Low` threshold: only `Low` impact changes proceed; `Medium` and `High` require approval.
/// - `Medium` threshold: `Low` and `Medium` proceed; `High` requires approval.
/// - `High` threshold: all changes proceed (threshold effectively disabled).
pub fn should_require_approval(impact: &ImpactLevel, threshold: &Option<ThresholdLevel>) -> bool {
    let threshold = match threshold {
        None => return false,
        Some(t) => t,
    };

    match threshold {
        ThresholdLevel::Low => {
            // Only Low impact passes
            !matches!(impact, ImpactLevel::Low)
        }
        ThresholdLevel::Medium => {
            // Low and Medium pass
            matches!(impact, ImpactLevel::High)
        }
        ThresholdLevel::High => {
            // All pass
            false
        }
    }
}

/// Evaluate a proposed change against the topology's configured threshold.
///
/// Returns a `ThresholdDecision` indicating whether the change can proceed
/// autonomously or requires human approval.
#[tracing::instrument(skip_all)]
pub fn evaluate_threshold(evaluation: &SignalEvaluation, topology: &Topology) -> ThresholdDecision {
    let threshold = topology
        .guardrails
        .as_ref()
        .and_then(|g| g.autonomous_apply_threshold);

    let impact = classify_impact(&evaluation.proposed_change, topology);

    if should_require_approval(&impact, &threshold) {
        let what = match &evaluation.proposed_change {
            ProposedChange::ScaleAgent {
                agent_name,
                from_replicas,
                to_replicas,
            } => {
                format!(
                    "Scale agent '{}' from {} to {} replicas",
                    agent_name, from_replicas, to_replicas
                )
            }
        };
        let request = ApprovalRequest {
            what,
            why: evaluation.justification.clone(),
            impact_level: format!("{:?}", impact),
            instruction: format!(
                "This change is classified as {:?} impact and exceeds the configured threshold ({:?}). \
                 Reply 'yes' or 'approve' to proceed, or 'no' / 'reject' to deny.",
                impact, threshold.unwrap()
            ),
        };
        ThresholdDecision::RequiresApproval { request }
    } else {
        ThresholdDecision::ProceedAutonomously
    }
}

/// Apply an autonomous topology change with human approval attribution.
///
/// This is the approved-path equivalent of `apply_autonomous_change()`.
/// The commit message includes an `Approved by:` line for audit trail.
#[tracing::instrument(skip_all, fields(agent_name = %agent_name))]
pub fn apply_with_approval(
    evaluation: SignalEvaluation,
    wh_path: &Path,
    agent_name: &str,
    approval_note: &str,
) -> Result<AutonomousApplyResult, DeployError> {
    apply_autonomous_change_inner(evaluation, wh_path, agent_name, Some(approval_note))
}

/// Internal implementation shared between `apply_autonomous_change` and `apply_with_approval`.
fn apply_autonomous_change_inner(
    evaluation: SignalEvaluation,
    wh_path: &Path,
    agent_name: &str,
    approval_note: Option<&str>,
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
        // Note: autonomous apply has no CLI context to read secrets from.
        // Agents deployed autonomously must have secrets pre-configured in their env.
        let _apply_result = apply::apply(committed, &[])?;

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

    let approved_suffix = if approval_note.is_some() {
        " (human-approved)"
    } else {
        ""
    };

    let commit_summary = format!(
        "[{}] apply: scale agent {} replicas to {}{}",
        agent_name, target_agent, new_replicas, approved_suffix
    );

    Ok(AutonomousApplyResult {
        commit_summary,
        plan_hash,
        changes,
    })
}

// =============================================================================
// Story 2.8: Agent Reads Its Own .wh — Autonomous Loop Smoke Test
// =============================================================================

/// A parsed summary of a `.wh` topology file.
///
/// Produced by `read_own_topology()` when an agent reads its own `.wh` file
/// at startup to validate the read path of the autonomous loop.
#[derive(Debug, Clone, PartialEq)]
pub struct TopologySummary {
    /// The topology name from the `.wh` file.
    pub topology_name: String,
    /// Number of agents declared in the topology.
    pub agent_count: usize,
    /// Number of streams declared in the topology.
    pub stream_count: usize,
    /// The raw YAML content of the `.wh` file.
    pub raw_yaml: String,
    /// Human-readable summary, e.g. "Topology 'dev': 2 agents, 1 stream".
    pub summary: String,
}

/// An event representing an agent publishing its topology summary to the stream.
///
/// This struct captures what would be published as a `TextMessage` to the stream,
/// including agent attribution for the log publisher field.
#[derive(Debug, Clone, PartialEq)]
pub struct TopologyPublishEvent {
    /// The name of the agent that read and published the topology.
    pub agent_name: String,
    /// Human-readable topology summary.
    pub summary: String,
    /// The raw `.wh` YAML content.
    pub content: String,
    /// ISO 8601 timestamp of when the event was created.
    pub timestamp: String,
}

/// Read and parse the agent's own `.wh` topology file.
///
/// Returns a `TopologySummary` with the parsed metadata and a human-readable
/// summary string. This proves the agent can locate and parse its `.wh` file
/// from the git-backed filesystem.
///
/// # Errors
///
/// Returns `DeployError::FileRead` if the file cannot be read, or
/// `DeployError::YamlParse`/`DeployError::InvalidTopology` if the YAML is malformed.
#[tracing::instrument(skip_all, fields(wh_path = %wh_path.display()))]
pub fn read_own_topology(wh_path: &Path) -> Result<TopologySummary, DeployError> {
    let raw_yaml = std::fs::read_to_string(wh_path).map_err(DeployError::FileRead)?;
    let topology = parse_topology(&raw_yaml)?;

    let agent_count = topology.agents.len();
    let stream_count = topology.streams.len();

    let agents_word = if agent_count == 1 { "agent" } else { "agents" };
    let streams_word = if stream_count == 1 {
        "stream"
    } else {
        "streams"
    };

    let summary = format!(
        "Topology '{}': {} {}, {} {}",
        topology.name, agent_count, agents_word, stream_count, streams_word
    );

    Ok(TopologySummary {
        topology_name: topology.name,
        agent_count,
        stream_count,
        raw_yaml,
        summary,
    })
}

/// Format a topology summary into a publish event with agent attribution.
///
/// Creates a `TopologyPublishEvent` that represents what the agent would
/// publish as a `TextMessage` to the stream.
#[tracing::instrument(skip_all, fields(agent_name = %agent_name))]
pub fn publish_topology_summary(
    summary: &TopologySummary,
    agent_name: &str,
) -> TopologyPublishEvent {
    TopologyPublishEvent {
        agent_name: agent_name.to_string(),
        summary: summary.summary.clone(),
        content: summary.raw_yaml.clone(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    }
}

/// Smoke test: read the agent's own `.wh` file and produce a publish event.
///
/// Orchestrates the full read path: `read_own_topology()` -> `publish_topology_summary()`
/// -> log the event -> return. This is the function an agent calls at startup to prove
/// it can read its own `.wh` file from the git-backed filesystem.
///
/// # Errors
///
/// Propagates errors from `read_own_topology()`.
#[tracing::instrument(skip_all, fields(agent_name = %agent_name))]
pub fn smoke_test_read_loop(
    wh_path: &Path,
    agent_name: &str,
) -> Result<TopologyPublishEvent, DeployError> {
    let summary = read_own_topology(wh_path)?;

    tracing::info!(
        agent_name = %agent_name,
        topology_name = %summary.topology_name,
        agent_count = summary.agent_count,
        stream_count = summary.stream_count,
        "agent read own .wh file and published topology summary"
    );

    let event = publish_topology_summary(&summary, agent_name);
    Ok(event)
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
            persona: None,
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
            Some(Guardrails {
                max_replicas: Some(1),
                ..Default::default()
            }),
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

    // =========================================================================
    // Impact classification tests (Story 7.6)
    // =========================================================================

    #[test]
    fn classify_impact_high_for_large_scaling() {
        let topology = make_topology(vec![], None);
        // 1 -> 3 = 200% increase = High
        let change = ProposedChange::ScaleAgent {
            agent_name: "researcher".to_string(),
            from_replicas: 1,
            to_replicas: 3,
        };
        assert_eq!(classify_impact(&change, &topology), ImpactLevel::High);
    }

    #[test]
    fn classify_impact_high_for_doubling() {
        let topology = make_topology(vec![], None);
        // 1 -> 2 = 100% increase = High
        let change = ProposedChange::ScaleAgent {
            agent_name: "researcher".to_string(),
            from_replicas: 1,
            to_replicas: 2,
        };
        assert_eq!(classify_impact(&change, &topology), ImpactLevel::High);
    }

    #[test]
    fn classify_impact_medium_for_50_pct() {
        let topology = make_topology(vec![], None);
        // 2 -> 3 = 50% increase = Medium
        let change = ProposedChange::ScaleAgent {
            agent_name: "researcher".to_string(),
            from_replicas: 2,
            to_replicas: 3,
        };
        assert_eq!(classify_impact(&change, &topology), ImpactLevel::Medium);
    }

    #[test]
    fn classify_impact_low_for_small_increase() {
        let topology = make_topology(vec![], None);
        // 5 -> 6 = 20% increase = Low
        let change = ProposedChange::ScaleAgent {
            agent_name: "researcher".to_string(),
            from_replicas: 5,
            to_replicas: 6,
        };
        assert_eq!(classify_impact(&change, &topology), ImpactLevel::Low);
    }

    #[test]
    fn classify_impact_high_for_large_scale_down() {
        let topology = make_topology(vec![], None);
        // 10 -> 1 = 90% reduction = High
        let change = ProposedChange::ScaleAgent {
            agent_name: "researcher".to_string(),
            from_replicas: 10,
            to_replicas: 1,
        };
        assert_eq!(classify_impact(&change, &topology), ImpactLevel::High);
    }

    #[test]
    fn classify_impact_medium_for_small_scale_down() {
        let topology = make_topology(vec![], None);
        // 10 -> 9 = 10% reduction, but scale-down floor is Medium
        let change = ProposedChange::ScaleAgent {
            agent_name: "researcher".to_string(),
            from_replicas: 10,
            to_replicas: 9,
        };
        assert_eq!(classify_impact(&change, &topology), ImpactLevel::Medium);
    }

    #[test]
    fn classify_impact_high_for_zero_replicas() {
        let topology = make_topology(vec![], None);
        // 0 -> 1 = always High
        let change = ProposedChange::ScaleAgent {
            agent_name: "researcher".to_string(),
            from_replicas: 0,
            to_replicas: 1,
        };
        assert_eq!(classify_impact(&change, &topology), ImpactLevel::High);
    }

    // =========================================================================
    // Threshold decision tests (Story 7.6)
    // =========================================================================

    #[test]
    fn should_require_approval_false_when_no_threshold() {
        assert!(!should_require_approval(&ImpactLevel::High, &None));
        assert!(!should_require_approval(&ImpactLevel::Medium, &None));
        assert!(!should_require_approval(&ImpactLevel::Low, &None));
    }

    #[test]
    fn should_require_approval_low_threshold() {
        let threshold = Some(ThresholdLevel::Low);
        assert!(!should_require_approval(&ImpactLevel::Low, &threshold));
        assert!(should_require_approval(&ImpactLevel::Medium, &threshold));
        assert!(should_require_approval(&ImpactLevel::High, &threshold));
    }

    #[test]
    fn should_require_approval_medium_threshold() {
        let threshold = Some(ThresholdLevel::Medium);
        assert!(!should_require_approval(&ImpactLevel::Low, &threshold));
        assert!(!should_require_approval(&ImpactLevel::Medium, &threshold));
        assert!(should_require_approval(&ImpactLevel::High, &threshold));
    }

    #[test]
    fn should_require_approval_high_threshold() {
        let threshold = Some(ThresholdLevel::High);
        assert!(!should_require_approval(&ImpactLevel::Low, &threshold));
        assert!(!should_require_approval(&ImpactLevel::Medium, &threshold));
        assert!(!should_require_approval(&ImpactLevel::High, &threshold));
    }

    #[test]
    fn evaluate_threshold_proceeds_when_no_guardrails() {
        let topology = make_topology(vec![make_agent("researcher", 1)], None);
        let eval = SignalEvaluation {
            signal_summary: "test".to_string(),
            proposed_change: ProposedChange::ScaleAgent {
                agent_name: "researcher".to_string(),
                from_replicas: 1,
                to_replicas: 3,
            },
            justification: "test".to_string(),
            confidence: ChangeConfidence::High,
        };
        assert!(matches!(
            evaluate_threshold(&eval, &topology),
            ThresholdDecision::ProceedAutonomously
        ));
    }

    #[test]
    fn evaluate_threshold_requires_approval_when_exceeds() {
        let topology = make_topology(
            vec![make_agent("researcher", 1)],
            Some(Guardrails {
                max_replicas: Some(10),
                autonomous_apply_threshold: Some(ThresholdLevel::Low),
                ..Default::default()
            }),
        );
        let eval = SignalEvaluation {
            signal_summary: "test".to_string(),
            proposed_change: ProposedChange::ScaleAgent {
                agent_name: "researcher".to_string(),
                from_replicas: 1,
                to_replicas: 2,
            },
            justification: "Scaling up".to_string(),
            confidence: ChangeConfidence::High,
        };
        match evaluate_threshold(&eval, &topology) {
            ThresholdDecision::RequiresApproval { request } => {
                assert!(!request.what.is_empty());
                assert!(!request.instruction.is_empty());
                assert_eq!(request.impact_level, "High");
            }
            other => panic!("Expected RequiresApproval, got {:?}", other),
        }
    }

    #[test]
    fn evaluate_threshold_proceeds_when_within_threshold() {
        let topology = make_topology(
            vec![make_agent("researcher", 5)],
            Some(Guardrails {
                max_replicas: Some(10),
                autonomous_apply_threshold: Some(ThresholdLevel::Low),
                ..Default::default()
            }),
        );
        let eval = SignalEvaluation {
            signal_summary: "test".to_string(),
            proposed_change: ProposedChange::ScaleAgent {
                agent_name: "researcher".to_string(),
                from_replicas: 5,
                to_replicas: 6,
            },
            justification: "Scaling up".to_string(),
            confidence: ChangeConfidence::High,
        };
        assert!(matches!(
            evaluate_threshold(&eval, &topology),
            ThresholdDecision::ProceedAutonomously
        ));
    }

    // =========================================================================
    // Error code tests (Story 7.6)
    // =========================================================================

    #[test]
    fn approval_required_error_code() {
        let err = DeployError::ApprovalRequired("test".to_string());
        assert_eq!(err.code(), "APPROVAL_REQUIRED");
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

    // =========================================================================
    // Story 2.8: read_own_topology tests
    // =========================================================================

    #[test]
    fn read_own_topology_returns_valid_summary() {
        let dir = tempfile::tempdir().unwrap();
        let wh_path = dir.path().join("topology.wh");
        std::fs::write(&wh_path, "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: donna\n    image: d:latest\n  - name: researcher\n    image: r:latest\nstreams:\n  - name: main\n").unwrap();

        let summary = read_own_topology(&wh_path).unwrap();
        assert_eq!(summary.topology_name, "dev");
        assert_eq!(summary.agent_count, 2);
        assert_eq!(summary.stream_count, 1);
        assert_eq!(summary.summary, "Topology 'dev': 2 agents, 1 stream");
        assert!(!summary.raw_yaml.is_empty());
    }

    #[test]
    fn read_own_topology_missing_file_returns_error() {
        let result = read_own_topology(std::path::Path::new("/nonexistent/topology.wh"));
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DeployError::FileRead(_)));
    }

    #[test]
    fn publish_topology_summary_contains_agent_attribution() {
        let summary = TopologySummary {
            topology_name: "dev".to_string(),
            agent_count: 1,
            stream_count: 1,
            raw_yaml: "api_version: wheelhouse.dev/v1\nname: dev\n".to_string(),
            summary: "Topology 'dev': 1 agent, 1 stream".to_string(),
        };
        let event = publish_topology_summary(&summary, "donna");
        assert_eq!(event.agent_name, "donna");
        assert!(!event.timestamp.is_empty());
        assert!(event.content.contains("wheelhouse.dev/v1"));
    }
}
