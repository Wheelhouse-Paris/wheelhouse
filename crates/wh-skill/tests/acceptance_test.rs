//! Acceptance tests for Story 5-2: SkillInvocation — Agent Invokes a Skill via Stream
//!
//! These tests verify the complete skill invocation pipeline:
//! - Allowlist validation (FM-05)
//! - SkillProgress emission (CM-06)
//! - SkillResult emission on success and failure

use std::collections::HashMap;
use wh_skill::allowlist::SkillAllowlist;
use wh_skill::config::{SkillRef, SkillsConfig};
use wh_skill::executor::SkillExecutorEvent;
use wh_skill::invocation::{
    build_skill_progress, build_skill_result_error, build_skill_result_success,
    SkillInvocationOutcome, SkillInvocationRequest,
};
use wh_skill::pipeline::InvocationPipeline;

// ── AC #1: Allowlist validation (FM-05) ─────────────────────────────

/// Given an agent publishes a SkillInvocation with a skill NOT in the allowlist,
/// When the pipeline processes it,
/// Then it rejects the invocation with a SkillResult error code "SKILL_NOT_PERMITTED".
#[tokio::test]
async fn test_disallowed_skill_is_rejected_with_error() {
    let allowlist = SkillAllowlist::new(vec!["summarize".to_string()]);

    let request = SkillInvocationRequest {
        skill_name: "web-search".to_string(),
        agent_id: "agent-1".to_string(),
        invocation_id: "inv-001".to_string(),
        parameters: HashMap::new(),
        timestamp_ms: 1710000000000,
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel(10);

    let pipeline = InvocationPipeline::new(allowlist, None, None);
    pipeline.process(request, tx).await.unwrap();

    let event = rx.recv().await.expect("should receive event");
    match event {
        SkillExecutorEvent::Completed {
            invocation_id,
            outcome,
        } => {
            assert_eq!(invocation_id, "inv-001");
            match outcome {
                SkillInvocationOutcome::Error {
                    error_code,
                    error_message,
                } => {
                    assert_eq!(error_code, "SKILL_NOT_PERMITTED");
                    assert!(error_message.contains("web-search"));
                }
                _ => panic!("Expected error outcome"),
            }
        }
        _ => panic!("Expected Completed event"),
    }
}

/// Given an agent publishes a SkillInvocation with a skill IN the allowlist,
/// When the pipeline processes it,
/// Then the allowlist check passes (no SKILL_NOT_PERMITTED error).
#[tokio::test]
async fn test_allowed_skill_passes_allowlist_check() {
    let allowlist = SkillAllowlist::new(vec!["summarize".to_string()]);
    assert!(allowlist.is_allowed("summarize"));
    assert!(!allowlist.is_allowed("web-search"));
}

// ── AC #2: SkillProgress emission (CM-06) ───────────────────────────

/// Given a valid SkillInvocation is published,
/// When the skill executor picks it up,
/// Then a SkillProgress object is published to indicate the skill has started.
#[tokio::test]
async fn test_skill_progress_emitted_before_result() {
    use git2::{Repository, Signature};
    use std::fs;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let git_repo = Repository::init(tmp.path()).unwrap();
    let sig = Signature::now("test", "test@test.com").unwrap();

    let skill_dir = tmp.path().join("summarize");
    let steps_dir = skill_dir.join("steps");
    fs::create_dir_all(&steps_dir).unwrap();
    fs::write(
        skill_dir.join("skill.md"),
        "---\nname: summarize\nversion: \"1.0.0\"\nsteps:\n  - steps/01-do.md\n---\n\n# Summarize\n",
    )
    .unwrap();
    fs::write(steps_dir.join("01-do.md"), "# Step 1\nSummarize the input.").unwrap();

    let mut index = git_repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = git_repo.find_tree(tree_oid).unwrap();
    let oid = git_repo
        .commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
        .unwrap();
    let commit = git_repo.find_commit(oid).unwrap();
    git_repo
        .tag_lightweight("v1.0.0", commit.as_object(), false)
        .unwrap();

    let allowlist = SkillAllowlist::new(vec!["summarize".to_string()]);
    let config = SkillsConfig {
        skills_repo: tmp.path().to_path_buf(),
        skills: vec![SkillRef {
            name: "summarize".into(),
            version: "1.0.0".into(),
        }],
    };
    let skill_repo = wh_skill::SkillRepository::open(tmp.path()).unwrap();
    let pipeline = InvocationPipeline::new(allowlist, Some(config), Some(skill_repo));

    let request = SkillInvocationRequest {
        skill_name: "summarize".to_string(),
        agent_id: "agent-1".to_string(),
        invocation_id: "inv-002".to_string(),
        parameters: HashMap::new(),
        timestamp_ms: 1710000000000,
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel(10);
    pipeline.process(request, tx).await.unwrap();

    // First event should be ProgressUpdate
    let first = rx.recv().await.expect("should receive progress event");
    assert!(
        matches!(first, SkillExecutorEvent::ProgressUpdate { .. }),
        "First event must be SkillProgress (CM-06)"
    );

    // Second event should be Completed with success
    let second = rx.recv().await.expect("should receive completed event");
    match second {
        SkillExecutorEvent::Completed {
            outcome,
            invocation_id,
        } => {
            assert_eq!(invocation_id, "inv-002");
            assert!(matches!(outcome, SkillInvocationOutcome::Success { .. }));
        }
        _ => panic!("Expected Completed event"),
    }
}

// ── AC #3: SkillResult with success ─────────────────────────────────

/// Given a skill completes successfully,
/// When the result is ready,
/// Then a SkillResult with success: true and output payload is published.
#[test]
fn test_skill_result_success_builder() {
    let result = build_skill_result_success("inv-003", "summarize", "Summary output text");
    assert_eq!(result.invocation_id, "inv-003");
    assert_eq!(result.skill_name, "summarize");
    assert!(result.success);
    assert_eq!(result.output, "Summary output text");
    assert!(result.error_message.is_empty());
    assert!(result.error_code.is_empty());
    assert!(result.timestamp_ms > 0);
}

/// Verify SkillResult error builder produces correct error codes (SCREAMING_SNAKE_CASE per SCV-01).
#[test]
fn test_skill_result_error_builder() {
    let result = build_skill_result_error(
        "inv-004",
        "web-search",
        "SKILL_NOT_PERMITTED",
        "Skill 'web-search' is not in agent's allowlist",
    );
    assert_eq!(result.invocation_id, "inv-004");
    assert_eq!(result.skill_name, "web-search");
    assert!(!result.success);
    assert_eq!(result.error_code, "SKILL_NOT_PERMITTED");
    assert!(result.error_message.contains("web-search"));
    assert!(result.timestamp_ms > 0);
}

/// Verify SkillProgress builder produces correct progress update.
#[test]
fn test_skill_progress_builder() {
    let progress = build_skill_progress("inv-005", "summarize", 0.0, "Skill execution started");
    assert_eq!(progress.invocation_id, "inv-005");
    assert_eq!(progress.skill_name, "summarize");
    assert!((progress.progress_percent - 0.0).abs() < f32::EPSILON);
    assert_eq!(progress.status_message, "Skill execution started");
    assert!(progress.timestamp_ms > 0);
}
