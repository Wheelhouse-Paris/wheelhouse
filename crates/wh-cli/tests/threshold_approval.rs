//! Acceptance tests for Story 7.6: Configurable Human Validation Threshold.
//!
//! These tests verify:
//! - AC #1: High-impact changes require approval when threshold is configured
//! - AC #2: Approved apply creates commit with approval attribution
//! - AC #3: Low-impact changes within threshold proceed autonomously
//! - AC #4: Pending approval expires after configurable timeout

#![allow(unused_must_use)]

/// Helper: create a temp directory with a git repo and a .wh file.
fn setup_workspace(wh_content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let wh_path = dir.path().join("topology.wh");
    std::fs::write(&wh_path, wh_content).unwrap();

    // Initialize git repo
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    (dir, wh_path)
}

// =============================================================================
// AC #1: High-impact changes require approval when threshold is configured
// =============================================================================

#[test]
fn high_impact_change_with_low_threshold_requires_approval() {
    use wh_broker::deploy::autonomous::{evaluate_signal, evaluate_threshold, ThresholdDecision};
    use wh_broker::deploy::{Agent, Guardrails, ThresholdLevel, Topology};

    let topology = Topology {
        api_version: "wheelhouse.dev/v1".to_string(),
        name: "dev".to_string(),
        agents: vec![Agent {
            name: "researcher".to_string(),
            image: "r:latest".to_string(),
            replicas: 1,
            streams: vec![],
            persona: None,
            skills_repo: None,
            skills: None,
        }],
        streams: vec![],
        surfaces: vec![],
        guardrails: Some(Guardrails {
            max_replicas: Some(10),
            autonomous_apply_threshold: Some(ThresholdLevel::Low),
            ..Default::default()
        }),
    };

    // Signal triggers a scale from 1->2 = 100% increase = High impact
    let eval = evaluate_signal("4 daily timeouts on researcher", &topology).unwrap();
    let decision = evaluate_threshold(&eval, &topology);

    match decision {
        ThresholdDecision::RequiresApproval { request } => {
            assert!(!request.what.is_empty());
            assert!(!request.instruction.is_empty());
            assert_eq!(request.impact_level, "High");
        }
        other => panic!("Expected RequiresApproval, got {other:?}"),
    }
}

#[test]
fn topology_with_threshold_field_parses_correctly() {
    use wh_broker::deploy::{parse_topology, ThresholdLevel};

    let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
agents:
  - name: researcher
    image: r:latest
guardrails:
  max_replicas: 5
  autonomous_apply_threshold: low
  approval_timeout_secs: 3600
"#;
    let topo = parse_topology(yaml).unwrap();
    let guardrails = topo.guardrails.unwrap();
    assert_eq!(
        guardrails.autonomous_apply_threshold,
        Some(ThresholdLevel::Low)
    );
    assert_eq!(guardrails.approval_timeout_secs, Some(3600));
}

#[test]
fn topology_without_threshold_defaults_to_no_approval() {
    use wh_broker::deploy::parse_topology;

    let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
agents:
  - name: researcher
    image: r:latest
"#;
    let topo = parse_topology(yaml).unwrap();
    assert!(topo.guardrails.is_none());
}

#[test]
fn topology_with_medium_threshold_parses() {
    use wh_broker::deploy::{parse_topology, ThresholdLevel};

    let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
guardrails:
  autonomous_apply_threshold: medium
"#;
    let topo = parse_topology(yaml).unwrap();
    let guardrails = topo.guardrails.unwrap();
    assert_eq!(
        guardrails.autonomous_apply_threshold,
        Some(ThresholdLevel::Medium)
    );
}

#[test]
fn topology_with_high_threshold_parses() {
    use wh_broker::deploy::{parse_topology, ThresholdLevel};

    let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
guardrails:
  autonomous_apply_threshold: high
"#;
    let topo = parse_topology(yaml).unwrap();
    let guardrails = topo.guardrails.unwrap();
    assert_eq!(
        guardrails.autonomous_apply_threshold,
        Some(ThresholdLevel::High)
    );
}

#[test]
fn topology_with_invalid_threshold_fails_parse() {
    use wh_broker::deploy::parse_topology;

    let yaml = r#"
api_version: wheelhouse.dev/v1
name: dev
guardrails:
  autonomous_apply_threshold: extreme
"#;
    let result = parse_topology(yaml);
    assert!(
        result.is_err(),
        "Invalid threshold value should fail to parse"
    );
}

// =============================================================================
// AC #3: Low-impact changes within threshold proceed autonomously
// =============================================================================

#[test]
fn low_impact_change_with_low_threshold_proceeds_autonomously() {
    use wh_broker::deploy::autonomous::{
        evaluate_threshold, ChangeConfidence, ProposedChange, SignalEvaluation, ThresholdDecision,
    };
    use wh_broker::deploy::{Agent, Guardrails, ThresholdLevel, Topology};

    let topology = Topology {
        api_version: "wheelhouse.dev/v1".to_string(),
        name: "dev".to_string(),
        agents: vec![Agent {
            name: "researcher".to_string(),
            image: "r:latest".to_string(),
            replicas: 5,
            streams: vec![],
            persona: None,
            skills_repo: None,
            skills: None,
        }],
        streams: vec![],
        surfaces: vec![],
        guardrails: Some(Guardrails {
            max_replicas: Some(10),
            autonomous_apply_threshold: Some(ThresholdLevel::Low),
            ..Default::default()
        }),
    };

    // 5 -> 6 = 20% increase = Low impact
    let eval = SignalEvaluation {
        signal_summary: "3 daily timeouts on researcher".to_string(),
        proposed_change: ProposedChange::ScaleAgent {
            agent_name: "researcher".to_string(),
            from_replicas: 5,
            to_replicas: 6,
        },
        justification: "Scaling up by 1".to_string(),
        confidence: ChangeConfidence::High,
    };

    let decision = evaluate_threshold(&eval, &topology);
    assert!(matches!(decision, ThresholdDecision::ProceedAutonomously));
}

#[test]
fn any_change_without_threshold_proceeds_autonomously() {
    use wh_broker::deploy::autonomous::{
        evaluate_threshold, ChangeConfidence, ProposedChange, SignalEvaluation, ThresholdDecision,
    };
    use wh_broker::deploy::{Agent, Topology};

    let topology = Topology {
        api_version: "wheelhouse.dev/v1".to_string(),
        name: "dev".to_string(),
        agents: vec![Agent {
            name: "researcher".to_string(),
            image: "r:latest".to_string(),
            replicas: 1,
            streams: vec![],
            persona: None,
            skills_repo: None,
            skills: None,
        }],
        streams: vec![],
        surfaces: vec![],
        guardrails: None,
    };

    let eval = SignalEvaluation {
        signal_summary: "4 daily timeouts on researcher".to_string(),
        proposed_change: ProposedChange::ScaleAgent {
            agent_name: "researcher".to_string(),
            from_replicas: 1,
            to_replicas: 2,
        },
        justification: "Scaling up".to_string(),
        confidence: ChangeConfidence::High,
    };

    let decision = evaluate_threshold(&eval, &topology);
    assert!(matches!(decision, ThresholdDecision::ProceedAutonomously));
}

#[test]
fn medium_threshold_allows_medium_but_blocks_high() {
    use wh_broker::deploy::autonomous::{
        evaluate_threshold, ChangeConfidence, ProposedChange, SignalEvaluation, ThresholdDecision,
    };
    use wh_broker::deploy::{Agent, Guardrails, ThresholdLevel, Topology};

    let topology = Topology {
        api_version: "wheelhouse.dev/v1".to_string(),
        name: "dev".to_string(),
        agents: vec![Agent {
            name: "researcher".to_string(),
            image: "r:latest".to_string(),
            replicas: 2,
            streams: vec![],
            persona: None,
            skills_repo: None,
            skills: None,
        }],
        streams: vec![],
        surfaces: vec![],
        guardrails: Some(Guardrails {
            max_replicas: Some(10),
            autonomous_apply_threshold: Some(ThresholdLevel::Medium),
            ..Default::default()
        }),
    };

    // 2 -> 3 = 50% = Medium impact -> should proceed with medium threshold
    let eval_medium = SignalEvaluation {
        signal_summary: "test".to_string(),
        proposed_change: ProposedChange::ScaleAgent {
            agent_name: "researcher".to_string(),
            from_replicas: 2,
            to_replicas: 3,
        },
        justification: "test".to_string(),
        confidence: ChangeConfidence::High,
    };
    assert!(matches!(
        evaluate_threshold(&eval_medium, &topology),
        ThresholdDecision::ProceedAutonomously
    ));

    // 1 -> 2 = 100% = High impact -> should require approval with medium threshold
    let eval_high = SignalEvaluation {
        signal_summary: "test".to_string(),
        proposed_change: ProposedChange::ScaleAgent {
            agent_name: "researcher".to_string(),
            from_replicas: 1,
            to_replicas: 2,
        },
        justification: "test".to_string(),
        confidence: ChangeConfidence::High,
    };
    assert!(matches!(
        evaluate_threshold(&eval_high, &topology),
        ThresholdDecision::RequiresApproval { .. }
    ));
}

#[test]
fn high_threshold_allows_all_changes() {
    use wh_broker::deploy::autonomous::{
        evaluate_threshold, ChangeConfidence, ProposedChange, SignalEvaluation, ThresholdDecision,
    };
    use wh_broker::deploy::{Agent, Guardrails, ThresholdLevel, Topology};

    let topology = Topology {
        api_version: "wheelhouse.dev/v1".to_string(),
        name: "dev".to_string(),
        agents: vec![Agent {
            name: "researcher".to_string(),
            image: "r:latest".to_string(),
            replicas: 1,
            streams: vec![],
            persona: None,
            skills_repo: None,
            skills: None,
        }],
        streams: vec![],
        surfaces: vec![],
        guardrails: Some(Guardrails {
            max_replicas: Some(10),
            autonomous_apply_threshold: Some(ThresholdLevel::High),
            ..Default::default()
        }),
    };

    // 1 -> 3 = 200% = High impact -> should still proceed with high threshold
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

// =============================================================================
// AC #2: Approved apply creates commit with approval attribution
// =============================================================================

#[test]
fn apply_with_approval_creates_attributed_commit() {
    use wh_broker::deploy::autonomous::{apply_with_approval, evaluate_signal};
    use wh_broker::deploy::load_topology;

    let (dir, wh_path) = setup_workspace(
        "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\n    replicas: 1\n",
    );

    let topology = load_topology(&wh_path).unwrap();
    let evaluation = evaluate_signal("4 daily timeouts on researcher", &topology).unwrap();

    let result = apply_with_approval(
        evaluation,
        &wh_path,
        "donna",
        "operator approved via Telegram",
    );
    assert!(result.is_ok());

    let result = result.unwrap();
    assert!(
        result.commit_summary.contains("human-approved"),
        "Commit summary should contain 'human-approved': {}",
        result.commit_summary
    );

    // Verify git log shows agent attribution
    let git_log = std::process::Command::new("git")
        .args(["log", "--oneline", "-1"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let log_text = String::from_utf8_lossy(&git_log.stdout);
    assert!(
        log_text.contains("[donna]"),
        "Git log should contain agent name: {log_text}"
    );
}

// =============================================================================
// AC #4: Pending approval expires after timeout
// =============================================================================

#[test]
fn pending_approval_expires_after_timeout() {
    use std::time::{Duration, Instant};
    use wh_broker::deploy::approval::{is_expired, PendingApproval};

    let pending = PendingApproval {
        id: "test-1".to_string(),
        agent_name: "donna".to_string(),
        requested_at: Instant::now() - Duration::from_secs(100),
        timeout: Duration::from_secs(60),
        wh_path: std::path::PathBuf::from("/tmp/test.wh"),
    };

    assert!(
        is_expired(&pending),
        "Approval should be expired after timeout"
    );
}

#[test]
fn pending_approval_not_expired_within_timeout() {
    use std::time::{Duration, Instant};
    use wh_broker::deploy::approval::{is_expired, PendingApproval};

    let pending = PendingApproval {
        id: "test-2".to_string(),
        agent_name: "donna".to_string(),
        requested_at: Instant::now(),
        timeout: Duration::from_secs(3600),
        wh_path: std::path::PathBuf::from("/tmp/test.wh"),
    };

    assert!(
        !is_expired(&pending),
        "Approval should not be expired within timeout"
    );
}

// =============================================================================
// Impact classification integration tests
// =============================================================================

#[test]
fn classify_impact_high_for_large_scaling() {
    use wh_broker::deploy::autonomous::{classify_impact, ImpactLevel, ProposedChange};
    use wh_broker::deploy::Topology;

    let topology = Topology {
        api_version: "wheelhouse.dev/v1".to_string(),
        name: "dev".to_string(),
        agents: vec![],
        streams: vec![],
        surfaces: vec![],
        guardrails: None,
    };

    // 1 -> 3 = 200% increase = High
    let change = ProposedChange::ScaleAgent {
        agent_name: "researcher".to_string(),
        from_replicas: 1,
        to_replicas: 3,
    };

    let impact = classify_impact(&change, &topology);
    assert_eq!(impact, ImpactLevel::High);
}

#[test]
fn classify_impact_low_for_small_increase() {
    use wh_broker::deploy::autonomous::{classify_impact, ImpactLevel, ProposedChange};
    use wh_broker::deploy::Topology;

    let topology = Topology {
        api_version: "wheelhouse.dev/v1".to_string(),
        name: "dev".to_string(),
        agents: vec![],
        streams: vec![],
        surfaces: vec![],
        guardrails: None,
    };

    // 5 -> 6 = 20% = Low
    let change = ProposedChange::ScaleAgent {
        agent_name: "researcher".to_string(),
        from_replicas: 5,
        to_replicas: 6,
    };

    let impact = classify_impact(&change, &topology);
    assert_eq!(impact, ImpactLevel::Low);
}

// =============================================================================
// Approval response parsing integration tests
// =============================================================================

#[test]
fn parse_approval_response_recognizes_yes() {
    use wh_broker::deploy::approval::{parse_approval_response, ApprovalResponse};

    assert!(matches!(
        parse_approval_response("yes"),
        ApprovalResponse::Approved
    ));
    assert!(matches!(
        parse_approval_response("YES"),
        ApprovalResponse::Approved
    ));
    assert!(matches!(
        parse_approval_response("approve"),
        ApprovalResponse::Approved
    ));
    assert!(matches!(
        parse_approval_response("Approved"),
        ApprovalResponse::Approved
    ));
    assert!(matches!(
        parse_approval_response("ok"),
        ApprovalResponse::Approved
    ));
}

#[test]
fn parse_approval_response_recognizes_no() {
    use wh_broker::deploy::approval::{parse_approval_response, ApprovalResponse};

    assert!(matches!(
        parse_approval_response("no"),
        ApprovalResponse::Rejected
    ));
    assert!(matches!(
        parse_approval_response("NO"),
        ApprovalResponse::Rejected
    ));
    assert!(matches!(
        parse_approval_response("reject"),
        ApprovalResponse::Rejected
    ));
    assert!(matches!(
        parse_approval_response("denied"),
        ApprovalResponse::Rejected
    ));
}

#[test]
fn parse_approval_response_unrecognized_for_arbitrary_text() {
    use wh_broker::deploy::approval::{parse_approval_response, ApprovalResponse};

    assert!(matches!(
        parse_approval_response("maybe later"),
        ApprovalResponse::Unrecognized(_)
    ));
    assert!(matches!(
        parse_approval_response("hello world"),
        ApprovalResponse::Unrecognized(_)
    ));
}

// =============================================================================
// Error code tests
// =============================================================================

#[test]
fn approval_required_error_code_is_correct() {
    use wh_broker::deploy::DeployError;

    let err = DeployError::ApprovalRequired("test".to_string());
    assert_eq!(err.code(), "APPROVAL_REQUIRED");
}

#[test]
fn approval_error_codes_are_screaming_snake_case() {
    use wh_broker::deploy::approval::ApprovalError;

    let timeout = ApprovalError::Timeout("test".to_string());
    assert_eq!(timeout.code(), "APPROVAL_TIMEOUT");

    let rejected = ApprovalError::Rejected("test".to_string());
    assert_eq!(rejected.code(), "APPROVAL_REJECTED");

    let invalid = ApprovalError::InvalidResponse("test".to_string());
    assert_eq!(invalid.code(), "INVALID_APPROVAL_RESPONSE");
}
