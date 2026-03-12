//! Integration tests for skill invocation pipeline (Story 5-2).
//!
//! Tests the complete flow: allowlist validation, version resolution,
//! skill loading from git, and execution with event ordering.

use std::collections::HashMap;
use std::fs;

use git2::{Repository, Signature};
use tempfile::TempDir;
use tokio::sync::mpsc;

use wh_skill::allowlist::SkillAllowlist;
use wh_skill::config::{SkillRef, SkillsConfig};
use wh_skill::executor::SkillExecutorEvent;
use wh_skill::invocation::{SkillInvocationOutcome, SkillInvocationRequest};
use wh_skill::pipeline::InvocationPipeline;
use wh_skill::repository::SkillRepository;

/// Helper: create a temp git repo with a skill, commit, and tag.
fn create_repo_with_skill(skill_name: &str, version: &str) -> (TempDir, git2::Oid) {
    let tmp = TempDir::new().unwrap();
    let repo = Repository::init(tmp.path()).unwrap();
    let sig = Signature::now("test", "test@test.com").unwrap();

    let skill_dir = tmp.path().join(skill_name);
    let steps_dir = skill_dir.join("steps");
    fs::create_dir_all(&steps_dir).unwrap();

    fs::write(
        skill_dir.join("skill.md"),
        format!(
            "---\nname: {skill_name}\nversion: \"{version}\"\nsteps:\n  - steps/01-do.md\n---\n\n# {skill_name}\n"
        ),
    )
    .unwrap();
    fs::write(
        steps_dir.join("01-do.md"),
        format!("# Step 1\nExecute {skill_name}.\n"),
    )
    .unwrap();

    let mut index = repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    let oid = repo
        .commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
        .unwrap();
    let commit = repo.find_commit(oid).unwrap();
    repo.tag_lightweight(&format!("v{version}"), commit.as_object(), false)
        .unwrap();

    (tmp, oid)
}

/// AC #1, #2, #3: Full pipeline — allowed skill produces ProgressUpdate then Completed(Success).
#[tokio::test]
async fn pipeline_processes_allowed_skill_end_to_end() {
    let (tmp, _oid) = create_repo_with_skill("summarize", "1.0.0");

    let allowlist = SkillAllowlist::new(vec!["summarize".into()]);
    let config = SkillsConfig {
        skills_repo: tmp.path().to_path_buf(),
        skills: vec![SkillRef {
            name: "summarize".into(),
            version: "1.0.0".into(),
        }],
    };
    let skill_repo = SkillRepository::open(tmp.path()).unwrap();
    let pipeline = InvocationPipeline::new(allowlist, Some(config), Some(skill_repo));

    let request = SkillInvocationRequest {
        skill_name: "summarize".into(),
        agent_id: "agent-1".into(),
        invocation_id: "inv-int-001".into(),
        parameters: HashMap::new(),
        timestamp_ms: 1710000000000,
    };

    let (tx, mut rx) = mpsc::channel(10);
    pipeline.process(request, tx).await.unwrap();

    // First: ProgressUpdate (CM-06)
    let first = rx.recv().await.unwrap();
    assert!(
        matches!(first, SkillExecutorEvent::ProgressUpdate { .. }),
        "first event must be ProgressUpdate"
    );

    // Second: Completed with Success
    let second = rx.recv().await.unwrap();
    match second {
        SkillExecutorEvent::Completed {
            invocation_id,
            outcome,
        } => {
            assert_eq!(invocation_id, "inv-int-001");
            match outcome {
                SkillInvocationOutcome::Success { output } => {
                    assert!(output.contains("Execute summarize"));
                }
                _ => panic!("expected Success outcome"),
            }
        }
        _ => panic!("expected Completed event"),
    }
}

/// AC #1: Disallowed skill emits error with code SKILL_NOT_PERMITTED.
#[tokio::test]
async fn pipeline_rejects_disallowed_skill() {
    let allowlist = SkillAllowlist::new(vec!["summarize".into()]);
    let pipeline = InvocationPipeline::new(allowlist, None, None);

    let request = SkillInvocationRequest {
        skill_name: "web-search".into(),
        agent_id: "agent-1".into(),
        invocation_id: "inv-int-002".into(),
        parameters: HashMap::new(),
        timestamp_ms: 1710000000000,
    };

    let (tx, mut rx) = mpsc::channel(10);
    pipeline.process(request, tx).await.unwrap();

    let event = rx.recv().await.unwrap();
    match event {
        SkillExecutorEvent::Completed { outcome, .. } => match outcome {
            SkillInvocationOutcome::Error {
                error_code,
                error_message,
            } => {
                assert_eq!(error_code, "SKILL_NOT_PERMITTED");
                assert!(error_message.contains("web-search"));
            }
            _ => panic!("expected Error outcome"),
        },
        _ => panic!("expected Completed event"),
    }
}

/// AC #1: Nonexistent skill in repo emits error with code SKILL_LOAD_FAILED.
#[tokio::test]
async fn pipeline_rejects_nonexistent_skill_in_repo() {
    let (tmp, _oid) = create_repo_with_skill("summarize", "1.0.0");

    let allowlist = SkillAllowlist::new(vec!["ghost-skill".into()]);
    let config = SkillsConfig {
        skills_repo: tmp.path().to_path_buf(),
        skills: vec![SkillRef {
            name: "ghost-skill".into(),
            version: "1.0.0".into(),
        }],
    };
    let skill_repo = SkillRepository::open(tmp.path()).unwrap();
    let pipeline = InvocationPipeline::new(allowlist, Some(config), Some(skill_repo));

    let request = SkillInvocationRequest {
        skill_name: "ghost-skill".into(),
        agent_id: "agent-1".into(),
        invocation_id: "inv-int-003".into(),
        parameters: HashMap::new(),
        timestamp_ms: 1710000000000,
    };

    let (tx, mut rx) = mpsc::channel(10);
    pipeline.process(request, tx).await.unwrap();

    let event = rx.recv().await.unwrap();
    match event {
        SkillExecutorEvent::Completed { outcome, .. } => match outcome {
            SkillInvocationOutcome::Error { error_code, .. } => {
                assert_eq!(error_code, "SKILL_LOAD_FAILED");
            }
            _ => panic!("expected Error outcome"),
        },
        _ => panic!("expected Completed event"),
    }
}

/// AC #2: SkillProgress is emitted BEFORE SkillResult (ordering guarantee).
#[tokio::test]
async fn skill_progress_emitted_before_skill_result() {
    let (tmp, _oid) = create_repo_with_skill("ordered", "1.0.0");

    let allowlist = SkillAllowlist::new(vec!["ordered".into()]);
    let config = SkillsConfig {
        skills_repo: tmp.path().to_path_buf(),
        skills: vec![SkillRef {
            name: "ordered".into(),
            version: "1.0.0".into(),
        }],
    };
    let skill_repo = SkillRepository::open(tmp.path()).unwrap();
    let pipeline = InvocationPipeline::new(allowlist, Some(config), Some(skill_repo));

    let request = SkillInvocationRequest {
        skill_name: "ordered".into(),
        agent_id: "agent-1".into(),
        invocation_id: "inv-int-004".into(),
        parameters: HashMap::new(),
        timestamp_ms: 1710000000000,
    };

    let (tx, mut rx) = mpsc::channel(10);
    pipeline.process(request, tx).await.unwrap();

    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    assert_eq!(events.len(), 2, "expected exactly 2 events");
    assert!(
        matches!(events[0], SkillExecutorEvent::ProgressUpdate { .. }),
        "first event must be ProgressUpdate"
    );
    assert!(
        matches!(events[1], SkillExecutorEvent::Completed { .. }),
        "second event must be Completed"
    );
}
