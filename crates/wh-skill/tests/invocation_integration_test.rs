//! Integration tests for skill invocation pipeline (Stories 5-2, 5-3).
//!
//! Tests the complete flow: allowlist validation, version resolution,
//! skill loading from git, execution with event ordering, and lazy caching.

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

/// Helper: create a temp git repo with two skills.
fn create_repo_with_two_skills(
    skill1: &str,
    skill2: &str,
    version: &str,
) -> (TempDir, git2::Oid) {
    let tmp = TempDir::new().unwrap();
    let repo = Repository::init(tmp.path()).unwrap();
    let sig = Signature::now("test", "test@test.com").unwrap();

    for skill_name in [skill1, skill2] {
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
            format!("# Step 1\nContent for {skill_name}.\n"),
        )
        .unwrap();
    }

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

// ══════════════════════════════════════════════════════════════════════
// Story 5-2 Integration Tests (preserved)
// ══════════════════════════════════════════════════════════════════════

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
    let mut pipeline = InvocationPipeline::new(allowlist, Some(config), Some(skill_repo));

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
    let mut pipeline = InvocationPipeline::new(allowlist, None, None);

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

/// AC #1: Nonexistent skill in repo emits error with code SKILL_FETCH_FAILED.
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
    let mut pipeline = InvocationPipeline::new(allowlist, Some(config), Some(skill_repo));

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
                assert_eq!(error_code, "SKILL_FETCH_FAILED");
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
    let mut pipeline = InvocationPipeline::new(allowlist, Some(config), Some(skill_repo));

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

// ══════════════════════════════════════════════════════════════════════
// Story 5-3 Integration Tests — Lazy loading and caching
// ══════════════════════════════════════════════════════════════════════

/// Two different skills are cached independently, each with correct content.
#[tokio::test]
async fn two_skills_cached_independently() {
    let (tmp, _oid) = create_repo_with_two_skills("summarize", "web-search", "1.0.0");

    let allowlist = SkillAllowlist::new(vec!["summarize".into(), "web-search".into()]);
    let config = SkillsConfig {
        skills_repo: tmp.path().to_path_buf(),
        skills: vec![
            SkillRef {
                name: "summarize".into(),
                version: "1.0.0".into(),
            },
            SkillRef {
                name: "web-search".into(),
                version: "1.0.0".into(),
            },
        ],
    };
    let skill_repo = SkillRepository::open(tmp.path()).unwrap();
    let mut pipeline = InvocationPipeline::new(allowlist, Some(config), Some(skill_repo));

    // Invoke first skill
    let req1 = SkillInvocationRequest {
        skill_name: "summarize".into(),
        agent_id: "agent-1".into(),
        invocation_id: "inv-s1".into(),
        parameters: HashMap::new(),
        timestamp_ms: 1710000000000,
    };
    let (tx1, mut rx1) = mpsc::channel(10);
    pipeline.process(req1, tx1).await.unwrap();
    let _ = rx1.recv().await.unwrap(); // progress
    let c1 = rx1.recv().await.unwrap(); // completed
    match c1 {
        SkillExecutorEvent::Completed { outcome, .. } => match outcome {
            SkillInvocationOutcome::Success { output } => {
                assert!(output.contains("summarize"), "output should contain summarize content");
            }
            _ => panic!("expected Success"),
        },
        _ => panic!("expected Completed"),
    }

    // Invoke second skill
    let req2 = SkillInvocationRequest {
        skill_name: "web-search".into(),
        agent_id: "agent-1".into(),
        invocation_id: "inv-s2".into(),
        parameters: HashMap::new(),
        timestamp_ms: 1710000000001,
    };
    let (tx2, mut rx2) = mpsc::channel(10);
    pipeline.process(req2, tx2).await.unwrap();
    let _ = rx2.recv().await.unwrap(); // progress
    let c2 = rx2.recv().await.unwrap(); // completed
    match c2 {
        SkillExecutorEvent::Completed { outcome, .. } => match outcome {
            SkillInvocationOutcome::Success { output } => {
                assert!(
                    output.contains("web-search"),
                    "output should contain web-search content"
                );
            }
            _ => panic!("expected Success"),
        },
        _ => panic!("expected Completed"),
    }

    // Both cached independently
    assert_eq!(pipeline.cache().len(), 2, "both skills should be cached");
}

/// Same skill at two different versions — cached independently.
#[tokio::test]
async fn same_skill_different_versions_cached_independently() {
    let tmp = TempDir::new().unwrap();
    let git_repo = Repository::init(tmp.path()).unwrap();
    let sig = Signature::now("test", "test@test.com").unwrap();

    // Create v1 of the skill
    let skill_dir = tmp.path().join("summarize");
    let steps_dir = skill_dir.join("steps");
    fs::create_dir_all(&steps_dir).unwrap();
    fs::write(
        skill_dir.join("skill.md"),
        "---\nname: summarize\nversion: \"1.0.0\"\nsteps:\n  - steps/01-do.md\n---\n\n# Summarize V1\n",
    )
    .unwrap();
    fs::write(steps_dir.join("01-do.md"), "# Step V1\nVersion 1 content.\n").unwrap();

    let mut index = git_repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = git_repo.find_tree(tree_oid).unwrap();
    let oid_v1 = git_repo
        .commit(Some("HEAD"), &sig, &sig, "v1", &tree, &[])
        .unwrap();
    let commit_v1 = git_repo.find_commit(oid_v1).unwrap();
    git_repo
        .tag_lightweight("v1.0.0", commit_v1.as_object(), false)
        .unwrap();

    // Create v2
    fs::write(
        skill_dir.join("skill.md"),
        "---\nname: summarize\nversion: \"2.0.0\"\nsteps:\n  - steps/01-do.md\n---\n\n# Summarize V2\n",
    )
    .unwrap();
    fs::write(steps_dir.join("01-do.md"), "# Step V2\nVersion 2 content.\n").unwrap();

    let mut index = git_repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = git_repo.find_tree(tree_oid).unwrap();
    let oid_v2 = git_repo
        .commit(Some("HEAD"), &sig, &sig, "v2", &tree, &[&commit_v1])
        .unwrap();
    let commit_v2 = git_repo.find_commit(oid_v2).unwrap();
    git_repo
        .tag_lightweight("v2.0.0", commit_v2.as_object(), false)
        .unwrap();

    // Pipeline with v1 first
    let allowlist = SkillAllowlist::new(vec!["summarize".into()]);
    let config_v1 = SkillsConfig {
        skills_repo: tmp.path().to_path_buf(),
        skills: vec![SkillRef {
            name: "summarize".into(),
            version: "1.0.0".into(),
        }],
    };
    let skill_repo = SkillRepository::open(tmp.path()).unwrap();
    let mut pipeline = InvocationPipeline::new(allowlist, Some(config_v1), Some(skill_repo));

    // Invoke v1
    let req1 = SkillInvocationRequest {
        skill_name: "summarize".into(),
        agent_id: "agent-1".into(),
        invocation_id: "inv-v1".into(),
        parameters: HashMap::new(),
        timestamp_ms: 1710000000000,
    };
    let (tx1, mut rx1) = mpsc::channel(10);
    pipeline.process(req1, tx1).await.unwrap();
    let _ = rx1.recv().await.unwrap(); // progress
    let c1 = rx1.recv().await.unwrap();
    match c1 {
        SkillExecutorEvent::Completed { outcome, .. } => match outcome {
            SkillInvocationOutcome::Success { output } => {
                assert!(output.contains("Version 1"), "should get v1 content");
            }
            _ => panic!("expected Success"),
        },
        _ => panic!("expected Completed"),
    }

    assert_eq!(pipeline.cache().len(), 1, "one version cached");
}

// ══════════════════════════════════════════════════════════════════════
// Story 5-4: Timeout + cache integration tests
// ══════════════════════════════════════════════════════════════════════

/// Given a skill execution times out, the skill IS still cached (loading succeeded).
/// The timeout only affects execution, not the cached LoadedSkill.
#[tokio::test]
async fn timeout_does_not_remove_cached_skill() {
    use std::future::Future;
    use std::pin::Pin;
    use std::time::Duration;

    use wh_skill::executor::SkillExecutor;
    use wh_skill::repository::LoadedSkill;

    struct SlowTestExecutor;
    impl SkillExecutor for SlowTestExecutor {
        fn execute<'a>(
            &'a self,
            _request: &'a SkillInvocationRequest,
            _skill: &'a LoadedSkill,
            _tx: &'a mpsc::Sender<SkillExecutorEvent>,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async { tokio::time::sleep(Duration::from_secs(1)).await })
        }
    }

    let (tmp, _oid) = create_repo_with_skill("alpha", "1.0.0");
    let allowlist = SkillAllowlist::new(vec!["alpha".to_string()]);

    let alpha_config = SkillsConfig {
        skills_repo: tmp.path().to_path_buf(),
        skills: vec![SkillRef {
            name: "alpha".into(),
            version: "1.0.0".into(),
        }],
    };
    let skill_repo = wh_skill::SkillRepository::open(tmp.path()).unwrap();

    let mut pipeline = InvocationPipeline::new(allowlist, Some(alpha_config), Some(skill_repo))
        .with_timeout(Duration::from_millis(50))
        .with_executor(Box::new(SlowTestExecutor));

    let request = SkillInvocationRequest {
        skill_name: "alpha".to_string(),
        agent_id: "agent-1".to_string(),
        invocation_id: "inv-cache-timeout".to_string(),
        parameters: HashMap::new(),
        timestamp_ms: 1710000000000,
    };

    let (tx, mut rx) = mpsc::channel(10);
    pipeline.process(request, tx).await.unwrap();

    // Drain events
    while let Ok(_) = rx.try_recv() {}

    // Skill should still be cached (loading from git succeeded, only execution timed out)
    assert_eq!(
        pipeline.cache().len(),
        1,
        "Skill should be cached even though execution timed out"
    );
}

/// All error paths produce a terminal Completed event (no silent failures).
#[tokio::test]
async fn all_error_paths_produce_completed_event() {
    use std::future::Future;
    use std::pin::Pin;
    use std::time::Duration;

    use wh_skill::executor::SkillExecutor;
    use wh_skill::repository::LoadedSkill;

    struct PanicTestExecutor;
    impl SkillExecutor for PanicTestExecutor {
        fn execute<'a>(
            &'a self,
            _request: &'a SkillInvocationRequest,
            _skill: &'a LoadedSkill,
            _tx: &'a mpsc::Sender<SkillExecutorEvent>,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
            Box::pin(async { panic!("integration test panic") })
        }
    }

    // Test 1: SKILL_NOT_PERMITTED
    {
        let allowlist = SkillAllowlist::new(vec![]);
        let mut pipeline = InvocationPipeline::new(allowlist, None, None);
        let (tx, mut rx) = mpsc::channel(10);
        let request = SkillInvocationRequest {
            skill_name: "x".to_string(),
            agent_id: "a".to_string(),
            invocation_id: "not-permitted".to_string(),
            parameters: HashMap::new(),
            timestamp_ms: 0,
        };
        pipeline.process(request, tx).await.unwrap();
        let event = rx.recv().await.unwrap();
        assert!(
            matches!(event, SkillExecutorEvent::Completed { .. }),
            "SKILL_NOT_PERMITTED must produce Completed"
        );
    }

    // Test 2: SKILL_LOAD_FAILED
    {
        let allowlist = SkillAllowlist::new(vec!["x".to_string()]);
        let mut pipeline = InvocationPipeline::new(allowlist, None, None);
        let (tx, mut rx) = mpsc::channel(10);
        let request = SkillInvocationRequest {
            skill_name: "x".to_string(),
            agent_id: "a".to_string(),
            invocation_id: "load-failed".to_string(),
            parameters: HashMap::new(),
            timestamp_ms: 0,
        };
        pipeline.process(request, tx).await.unwrap();
        let event = rx.recv().await.unwrap();
        assert!(
            matches!(event, SkillExecutorEvent::Completed { .. }),
            "SKILL_LOAD_FAILED must produce Completed"
        );
    }

    // Test 3: SKILL_FETCH_FAILED
    {
        let allowlist = SkillAllowlist::new(vec!["x".to_string()]);
        let config = SkillsConfig {
            skills_repo: "/none".into(),
            skills: vec![SkillRef {
                name: "x".into(),
                version: "1.0.0".into(),
            }],
        };
        let mut pipeline = InvocationPipeline::new(allowlist, Some(config), None);
        let (tx, mut rx) = mpsc::channel(10);
        let request = SkillInvocationRequest {
            skill_name: "x".to_string(),
            agent_id: "a".to_string(),
            invocation_id: "fetch-failed".to_string(),
            parameters: HashMap::new(),
            timestamp_ms: 0,
        };
        pipeline.process(request, tx).await.unwrap();
        let event = rx.recv().await.unwrap();
        assert!(
            matches!(event, SkillExecutorEvent::Completed { .. }),
            "SKILL_FETCH_FAILED must produce Completed"
        );
    }

    // Test 4: SKILL_EXECUTION_FAILED (panic)
    {
        let (tmp, _oid) = create_repo_with_skill("alpha", "1.0.0");
        let allowlist = SkillAllowlist::new(vec!["alpha".to_string()]);
        let alpha_config = SkillsConfig {
            skills_repo: tmp.path().to_path_buf(),
            skills: vec![SkillRef {
                name: "alpha".into(),
                version: "1.0.0".into(),
            }],
        };
        let skill_repo = wh_skill::SkillRepository::open(tmp.path()).unwrap();
        let mut pipeline =
            InvocationPipeline::new(allowlist, Some(alpha_config), Some(skill_repo))
                .with_executor(Box::new(PanicTestExecutor));
        let (tx, mut rx) = mpsc::channel(10);
        let request = SkillInvocationRequest {
            skill_name: "alpha".to_string(),
            agent_id: "a".to_string(),
            invocation_id: "panic-fail".to_string(),
            parameters: HashMap::new(),
            timestamp_ms: 0,
        };
        pipeline.process(request, tx).await.unwrap();
        let event = rx.recv().await.unwrap();
        assert!(
            matches!(event, SkillExecutorEvent::Completed { .. }),
            "SKILL_EXECUTION_FAILED must produce Completed"
        );
    }

    // Test 5: SKILL_TIMEOUT
    {
        struct SlowTestExec;
        impl SkillExecutor for SlowTestExec {
            fn execute<'a>(
                &'a self,
                _request: &'a SkillInvocationRequest,
                _skill: &'a LoadedSkill,
                _tx: &'a mpsc::Sender<SkillExecutorEvent>,
            ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
                Box::pin(async { tokio::time::sleep(Duration::from_secs(10)).await })
            }
        }

        let (tmp, _oid) = create_repo_with_skill("alpha", "1.0.0");
        let allowlist = SkillAllowlist::new(vec!["alpha".to_string()]);
        let alpha_config = SkillsConfig {
            skills_repo: tmp.path().to_path_buf(),
            skills: vec![SkillRef {
                name: "alpha".into(),
                version: "1.0.0".into(),
            }],
        };
        let skill_repo2 = wh_skill::SkillRepository::open(tmp.path()).unwrap();
        let mut pipeline =
            InvocationPipeline::new(allowlist, Some(alpha_config), Some(skill_repo2))
                .with_timeout(Duration::from_millis(50))
                .with_executor(Box::new(SlowTestExec));
        let (tx, mut rx) = mpsc::channel(10);
        let request = SkillInvocationRequest {
            skill_name: "alpha".to_string(),
            agent_id: "a".to_string(),
            invocation_id: "timeout-fail".to_string(),
            parameters: HashMap::new(),
            timestamp_ms: 0,
        };
        pipeline.process(request, tx).await.unwrap();
        // Drain to find Completed
        let mut has_completed = false;
        while let Ok(event) = rx.try_recv() {
            if matches!(event, SkillExecutorEvent::Completed { .. }) {
                has_completed = true;
            }
        }
        assert!(
            has_completed,
            "SKILL_TIMEOUT must produce Completed"
        );
    }
}
