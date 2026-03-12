//! Acceptance tests for Story 7.3: Full Autonomous Observe-Decide-Act Loop.
//!
//! These tests verify:
//! - AC #1: Agent autonomously generates and applies topology changes from signals
//! - AC #2: Git commits are attributed to the agent with justification and plan hash
//! - AC #3: Notification struct contains what-changed, why, and commit-ref
//! - AC #4: Self-destruct detection blocks agent from removing itself (CM-05)
//! - AC #5: All changes are attributed to specific agent decisions

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
// AC #4: Self-destruct detection (CM-05)
// =============================================================================

#[test]
fn self_destruct_detection_blocks_agent_removing_itself() {
    use wh_broker::deploy::lint::lint;
    use wh_broker::deploy::plan::plan_with_self_check;
    use wh_broker::deploy::DeployError;

    let (dir, wh_path) = setup_workspace(
        "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: donna\n    image: donna:latest\n  - name: researcher\n    image: r:latest\n",
    );

    // Set up current state with donna
    let wh_dir = dir.path().join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();
    std::fs::write(
        wh_dir.join("state.json"),
        r#"{"api_version":"wheelhouse.dev/v1","name":"dev","agents":[{"name":"donna","image":"donna:latest","replicas":1,"streams":[]},{"name":"researcher","image":"r:latest","replicas":1,"streams":[]}],"streams":[]}"#,
    ).unwrap();

    // Now write a new topology that removes donna
    std::fs::write(
        &wh_path,
        "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\n",
    ).unwrap();

    let linted = lint(&wh_path).unwrap();
    let result = plan_with_self_check(linted, Some("donna"));
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, DeployError::SelfDestructDetected(_)));
}

#[test]
fn self_destruct_detection_allows_removing_other_agents() {
    use wh_broker::deploy::lint::lint;
    use wh_broker::deploy::plan::plan_with_self_check;

    let (dir, wh_path) = setup_workspace(
        "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: donna\n    image: donna:latest\n",
    );

    let wh_dir = dir.path().join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();
    std::fs::write(
        wh_dir.join("state.json"),
        r#"{"api_version":"wheelhouse.dev/v1","name":"dev","agents":[{"name":"donna","image":"donna:latest","replicas":1,"streams":[]},{"name":"researcher","image":"r:latest","replicas":1,"streams":[]}],"streams":[]}"#,
    ).unwrap();

    let linted = lint(&wh_path).unwrap();
    let result = plan_with_self_check(linted, Some("donna"));
    assert!(result.is_ok());
}

#[test]
fn self_destruct_detection_skipped_in_operator_mode() {
    use wh_broker::deploy::lint::lint;
    use wh_broker::deploy::plan::plan_with_self_check;

    let (dir, wh_path) = setup_workspace("api_version: wheelhouse.dev/v1\nname: dev\nagents: []\n");

    let wh_dir = dir.path().join(".wh");
    std::fs::create_dir_all(&wh_dir).unwrap();
    std::fs::write(
        wh_dir.join("state.json"),
        r#"{"api_version":"wheelhouse.dev/v1","name":"dev","agents":[{"name":"donna","image":"donna:latest","replicas":1,"streams":[]}],"streams":[]}"#,
    ).unwrap();

    let linted = lint(&wh_path).unwrap();
    let result = plan_with_self_check(linted, None);
    assert!(result.is_ok());
}

#[test]
fn self_destruct_error_code_is_correct() {
    use wh_broker::deploy::DeployError;

    let err = DeployError::SelfDestructDetected("test".to_string());
    assert_eq!(err.code(), "SELF_DESTRUCT_DETECTED");
}

// =============================================================================
// AC #1: Autonomous signal evaluation and topology modification
// =============================================================================

#[test]
fn signal_evaluation_proposes_scale_up_on_timeout_pattern() {
    use wh_broker::deploy::autonomous::evaluate_signal;
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
        }],
        streams: vec![],
        guardrails: None,
    };

    let eval = evaluate_signal("4 daily timeouts on researcher", &topology);
    assert!(eval.is_some());
    let eval = eval.unwrap();
    assert!(!eval.justification.is_empty());
}

#[test]
fn signal_evaluation_returns_none_for_unrecognized_pattern() {
    use wh_broker::deploy::autonomous::evaluate_signal;
    use wh_broker::deploy::Topology;

    let topology = Topology {
        api_version: "wheelhouse.dev/v1".to_string(),
        name: "dev".to_string(),
        agents: vec![],
        streams: vec![],
        guardrails: None,
    };

    let eval = evaluate_signal("hello world nothing to see", &topology);
    assert!(eval.is_none());
}

#[test]
fn signal_evaluation_respects_guardrail_max_replicas() {
    use wh_broker::deploy::autonomous::evaluate_signal;
    use wh_broker::deploy::{Agent, Guardrails, Topology};

    let topology = Topology {
        api_version: "wheelhouse.dev/v1".to_string(),
        name: "dev".to_string(),
        agents: vec![Agent {
            name: "researcher".to_string(),
            image: "r:latest".to_string(),
            replicas: 1,
            streams: vec![],
            persona: None,
        }],
        streams: vec![],
        guardrails: Some(Guardrails {
            max_replicas: Some(1),
            ..Default::default()
        }),
    };

    let eval = evaluate_signal("4 daily timeouts on researcher", &topology);
    assert!(eval.is_none());
}

// =============================================================================
// AC #1: .wh file modification
// =============================================================================

#[test]
fn modify_topology_replicas_updates_target_agent() {
    use wh_broker::deploy::autonomous::modify_topology_replicas;

    let content = "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\n    replicas: 1\n";
    let modified = modify_topology_replicas(content, "researcher", 2).unwrap();

    let topo: serde_yaml::Value = serde_yaml::from_str(&modified).unwrap();
    let agents = topo["agents"].as_sequence().unwrap();
    let researcher = &agents[0];
    assert_eq!(researcher["replicas"].as_u64().unwrap(), 2);
}

#[test]
fn modify_topology_replicas_errors_on_nonexistent_agent() {
    use wh_broker::deploy::autonomous::modify_topology_replicas;

    let content = "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\n";
    let result = modify_topology_replicas(content, "nonexistent", 2);
    assert!(result.is_err());
}

#[test]
fn modify_topology_replicas_preserves_other_fields() {
    use wh_broker::deploy::autonomous::modify_topology_replicas;

    let content = "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\n    replicas: 1\n  - name: donna\n    image: donna:latest\n    replicas: 1\nstreams:\n  - name: main\n";
    let modified = modify_topology_replicas(content, "researcher", 3).unwrap();

    let topo: serde_yaml::Value = serde_yaml::from_str(&modified).unwrap();
    let agents = topo["agents"].as_sequence().unwrap();
    let donna = agents
        .iter()
        .find(|a| a["name"].as_str() == Some("donna"))
        .unwrap();
    assert_eq!(donna["replicas"].as_u64().unwrap(), 1);
    assert!(topo["streams"].as_sequence().unwrap().len() == 1);
}

// =============================================================================
// AC #1, #2, #5: Autonomous apply pipeline — full integration
// =============================================================================

#[test]
fn autonomous_apply_creates_attributed_git_commit() {
    use wh_broker::deploy::autonomous::{apply_autonomous_change, evaluate_signal};
    use wh_broker::deploy::load_topology;

    let (dir, wh_path) = setup_workspace(
        "api_version: wheelhouse.dev/v1\nname: dev\nagents:\n  - name: researcher\n    image: r:latest\n    replicas: 1\n",
    );

    let topology = load_topology(&wh_path).unwrap();
    let evaluation = evaluate_signal("4 daily timeouts on researcher", &topology).unwrap();
    let result = apply_autonomous_change(evaluation, &wh_path, "donna");
    assert!(result.is_ok());

    let result = result.unwrap();
    assert!(!result.plan_hash.is_empty());
    assert!(result.plan_hash.starts_with("sha256:"));
    assert!(!result.commit_summary.is_empty());
    assert!(result.commit_summary.contains("[donna]"));

    // Verify git log shows agent attribution (AC #2, #5)
    let git_log = std::process::Command::new("git")
        .args(["log", "--oneline", "-1"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let log_text = String::from_utf8_lossy(&git_log.stdout);
    assert!(
        log_text.contains("[donna]"),
        "Git log should contain agent name: {}",
        log_text
    );

    // Verify the commit message body contains plan hash (AC #2)
    let git_log_body = std::process::Command::new("git")
        .args(["log", "-1", "--format=%B"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let body = String::from_utf8_lossy(&git_log_body.stdout);
    assert!(
        body.contains("Plan: sha256:"),
        "Commit body should contain plan hash: {}",
        body
    );
}

// =============================================================================
// AC #3: Notification struct
// =============================================================================

#[test]
fn notification_contains_required_fields() {
    use wh_broker::deploy::autonomous::{
        format_notification, AutonomousApplyResult, ChangeConfidence, ProposedChange,
        SignalEvaluation,
    };

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
    assert!(
        !notification.what_changed.is_empty(),
        "what_changed should not be empty"
    );
    assert!(!notification.why.is_empty(), "why should not be empty");
    assert!(
        !notification.commit_ref.is_empty(),
        "commit_ref should not be empty"
    );
    assert!(
        notification.what_changed.contains("researcher"),
        "what_changed should mention agent"
    );
    assert_eq!(notification.commit_ref, "sha256:abc123");
}
