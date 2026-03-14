//! Acceptance tests for Stories 5-2, 5-3, and 5-4:
//! - 5-2: SkillInvocation — Agent Invokes a Skill via Stream
//! - 5-3: Lazy Skill Loading from Git
//! - 5-4: SkillResult Error Contract — No Silent Timeouts
//!
//! These tests verify the complete skill invocation pipeline:
//! - Allowlist validation (FM-05)
//! - SkillProgress emission (CM-06)
//! - SkillResult emission on success and failure
//! - Lazy loading with session cache (5-3)
//! - SKILL_FETCH_FAILED on unreachable repo (5-3)
//! - SKILL_TIMEOUT on execution timeout (5-4)
//! - SKILL_EXECUTION_FAILED on executor panic (5-4)

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use wh_skill::allowlist::SkillAllowlist;
use wh_skill::config::{SkillRef, SkillsConfig};
use wh_skill::executor::{SkillExecutor, SkillExecutorEvent};
use wh_skill::invocation::{
    build_skill_progress, build_skill_result_error, build_skill_result_success,
    SkillInvocationOutcome, SkillInvocationRequest,
};
use wh_skill::pipeline::InvocationPipeline;
use wh_skill::repository::LoadedSkill;

// ── AC #1 (5-2): Allowlist validation (FM-05) ───────────────────────

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

    let mut pipeline = InvocationPipeline::new(allowlist, None, None);
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

// ── AC #2 (5-2): SkillProgress emission (CM-06) ─────────────────────

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
    let mut pipeline = InvocationPipeline::new(allowlist, Some(config), Some(skill_repo));

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

// ── AC #3 (5-2): SkillResult with success ───────────────────────────

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

// ══════════════════════════════════════════════════════════════════════
// Story 5-3: Lazy Skill Loading from Git — Acceptance Tests
// ══════════════════════════════════════════════════════════════════════

// ── AC #1 (5-3): No skills loaded at startup ─────────────────────────

/// Given an InvocationPipeline is created with a skills repository,
/// When construction completes,
/// Then no skill files have been loaded (cache is empty).
#[tokio::test]
async fn test_no_skills_loaded_at_pipeline_construction() {
    let allowlist = SkillAllowlist::new(vec!["summarize".to_string()]);
    let pipeline = InvocationPipeline::new(allowlist, None, None);
    assert!(
        pipeline.cache().is_empty(),
        "Cache must be empty at construction — no startup loading"
    );
}

// ── AC #2 (5-3): On-demand fetch with session cache ──────────────────

/// Given a SkillInvocation for "summarize" is published,
/// When the skill executor processes it,
/// Then the skill is fetched from git on demand,
/// And a subsequent invocation of the same skill uses the local cache.
#[tokio::test]
async fn test_lazy_load_and_cache_hit() {
    use git2::{Repository, Signature};
    use std::fs;
    use tempfile::TempDir;

    // Set up a git repo with the "summarize" skill
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
    fs::write(steps_dir.join("01-do.md"), "# Step 1\nSummarize content.").unwrap();

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
    let mut pipeline = InvocationPipeline::new(allowlist, Some(config), Some(skill_repo));

    // Verify cache is empty before first invocation
    assert!(
        pipeline.cache().is_empty(),
        "cache must be empty before first invocation"
    );

    // First invocation — should fetch from git and populate cache
    let request1 = SkillInvocationRequest {
        skill_name: "summarize".to_string(),
        agent_id: "agent-1".to_string(),
        invocation_id: "inv-first".to_string(),
        parameters: HashMap::new(),
        timestamp_ms: 1710000000000,
    };
    let (tx1, mut rx1) = tokio::sync::mpsc::channel(10);
    pipeline.process(request1, tx1).await.unwrap();

    // Drain events (progress + completed)
    let _progress = rx1.recv().await.unwrap();
    let completed = rx1.recv().await.unwrap();
    assert!(
        matches!(
            completed,
            SkillExecutorEvent::Completed {
                outcome: SkillInvocationOutcome::Success { .. },
                ..
            }
        ),
        "First invocation should succeed"
    );

    // Cache should now have one entry
    assert_eq!(
        pipeline.cache().len(),
        1,
        "cache should have 1 entry after first invocation"
    );

    // Second invocation — should use cache (same skill, same version)
    let request2 = SkillInvocationRequest {
        skill_name: "summarize".to_string(),
        agent_id: "agent-1".to_string(),
        invocation_id: "inv-second".to_string(),
        parameters: HashMap::new(),
        timestamp_ms: 1710000000001,
    };
    let (tx2, mut rx2) = tokio::sync::mpsc::channel(10);
    pipeline.process(request2, tx2).await.unwrap();

    // Drain events
    let _progress2 = rx2.recv().await.unwrap();
    let completed2 = rx2.recv().await.unwrap();
    assert!(
        matches!(
            completed2,
            SkillExecutorEvent::Completed {
                outcome: SkillInvocationOutcome::Success { .. },
                ..
            }
        ),
        "Second invocation should also succeed (from cache)"
    );

    // Cache should still have exactly one entry (no duplicate)
    assert_eq!(
        pipeline.cache().len(),
        1,
        "cache should still have 1 entry — same skill reused from cache"
    );
}

// ── AC #3 (5-3): SKILL_FETCH_FAILED on unreachable repo ─────────────

/// Given the skills git repository is unreachable (None),
/// When a SkillInvocation is published,
/// Then a SkillResult error is published immediately with code SKILL_FETCH_FAILED.
#[tokio::test]
async fn test_unreachable_repo_returns_skill_fetch_failed() {
    let allowlist = SkillAllowlist::new(vec!["web-search".to_string()]);
    let config = SkillsConfig {
        skills_repo: "/nonexistent/path".into(),
        skills: vec![SkillRef {
            name: "web-search".into(),
            version: "1.0.0".into(),
        }],
    };
    // repo is None — simulates unreachable repository
    let mut pipeline = InvocationPipeline::new(allowlist, Some(config), None);

    let request = SkillInvocationRequest {
        skill_name: "web-search".to_string(),
        agent_id: "agent-1".to_string(),
        invocation_id: "inv-fetch-fail".to_string(),
        parameters: HashMap::new(),
        timestamp_ms: 1710000000000,
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel(10);
    pipeline.process(request, tx).await.unwrap();

    let event = rx.recv().await.expect("should receive error event");
    match event {
        SkillExecutorEvent::Completed {
            invocation_id,
            outcome,
        } => {
            assert_eq!(invocation_id, "inv-fetch-fail");
            match outcome {
                SkillInvocationOutcome::Error {
                    error_code,
                    error_message,
                } => {
                    assert_eq!(
                        error_code, "SKILL_FETCH_FAILED",
                        "Error code must be SKILL_FETCH_FAILED for unreachable repo"
                    );
                    assert!(error_message.contains("web-search"));
                }
                _ => panic!("Expected error outcome"),
            }
        }
        _ => panic!("Expected Completed event"),
    }
}

// ══════════════════════════════════════════════════════════════════════
// Story 5-4: SkillResult Error Contract — No Silent Timeouts
// ══════════════════════════════════════════════════════════════════════

/// Helper: create a git repo with a skill for use in 5-4 tests.
fn create_test_git_repo_with_skill() -> (tempfile::TempDir, SkillsConfig, wh_skill::SkillRepository)
{
    use git2::{Repository, Signature};
    use std::fs;

    let tmp = tempfile::TempDir::new().unwrap();
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
    fs::write(steps_dir.join("01-do.md"), "# Step 1\nDo the thing.").unwrap();

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

    let config = SkillsConfig {
        skills_repo: tmp.path().to_path_buf(),
        skills: vec![SkillRef {
            name: "summarize".into(),
            version: "1.0.0".into(),
        }],
    };
    let skill_repo = wh_skill::SkillRepository::open(tmp.path()).unwrap();
    (tmp, config, skill_repo)
}

/// A slow executor that sleeps for a configurable duration before completing.
/// Used to test the timeout mechanism.
struct SlowExecutor {
    sleep_duration: Duration,
}

impl SkillExecutor for SlowExecutor {
    fn execute<'a>(
        &'a self,
        request: &'a SkillInvocationRequest,
        _skill: &'a LoadedSkill,
        tx: &'a tokio::sync::mpsc::Sender<SkillExecutorEvent>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        let sleep_duration = self.sleep_duration;
        Box::pin(async move {
            // Emit progress before sleeping (CM-06)
            let _ = tx
                .send(SkillExecutorEvent::ProgressUpdate {
                    invocation_id: request.invocation_id.clone(),
                    progress_percent: 0.0,
                    status_message: "Slow skill started".into(),
                })
                .await;

            // Sleep to simulate long execution
            tokio::time::sleep(sleep_duration).await;

            // This should NOT be reached if timeout fires
            let _ = tx
                .send(SkillExecutorEvent::Completed {
                    invocation_id: request.invocation_id.clone(),
                    outcome: SkillInvocationOutcome::Success {
                        output: "slow skill completed".into(),
                    },
                })
                .await;
        })
    }
}

/// A panicking executor for testing SKILL_EXECUTION_FAILED.
struct PanickingExecutor;

impl SkillExecutor for PanickingExecutor {
    fn execute<'a>(
        &'a self,
        _request: &'a SkillInvocationRequest,
        _skill: &'a LoadedSkill,
        _tx: &'a tokio::sync::mpsc::Sender<SkillExecutorEvent>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            panic!("executor panic: simulated crash");
        })
    }
}

// ── AC #1 (5-4): Skill execution timeout produces SKILL_TIMEOUT ────

/// Given a skill execution times out (exceeds configured timeout),
/// When the timeout fires,
/// Then a SkillResult with success: false and error_code: SKILL_TIMEOUT is published,
/// And no invocation is left in a pending state with no response.
#[tokio::test]
async fn test_skill_timeout_produces_skill_timeout_result() {
    let (_tmp, config, skill_repo) = create_test_git_repo_with_skill();
    let allowlist = SkillAllowlist::new(vec!["summarize".to_string()]);

    // Create pipeline with a very short timeout (100ms) and a slow executor (500ms)
    let mut pipeline = InvocationPipeline::new(allowlist, Some(config), Some(skill_repo))
        .with_timeout(Duration::from_millis(100))
        .with_executor(Box::new(SlowExecutor {
            sleep_duration: Duration::from_millis(500),
        }));

    let request = SkillInvocationRequest {
        skill_name: "summarize".to_string(),
        agent_id: "agent-1".to_string(),
        invocation_id: "inv-timeout".to_string(),
        parameters: HashMap::new(),
        timestamp_ms: 1710000000000,
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel(10);
    pipeline.process(request, tx).await.unwrap();

    // Collect all events — should get progress (from before sleep) then timeout error
    let mut events = vec![];
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    // The last event must be a Completed with SKILL_TIMEOUT
    let last = events.last().expect("should have at least one event");
    match last {
        SkillExecutorEvent::Completed {
            invocation_id,
            outcome,
        } => {
            assert_eq!(invocation_id, "inv-timeout");
            match outcome {
                SkillInvocationOutcome::Error {
                    error_code,
                    error_message,
                } => {
                    assert_eq!(
                        error_code, "SKILL_TIMEOUT",
                        "Error code must be SKILL_TIMEOUT"
                    );
                    assert!(
                        error_message.contains("summarize"),
                        "Error message should contain skill name"
                    );
                    assert!(
                        error_message.contains("timed out"),
                        "Error message should mention timeout"
                    );
                }
                _ => panic!("Expected Error outcome, got Success"),
            }
        }
        _ => panic!("Last event must be Completed, got {last:?}"),
    }
}

/// Verify that no invocation is left pending after a timeout — there is always
/// a terminal Completed event.
#[tokio::test]
async fn test_no_pending_invocation_after_timeout() {
    let (_tmp, config, skill_repo) = create_test_git_repo_with_skill();
    let allowlist = SkillAllowlist::new(vec!["summarize".to_string()]);

    let mut pipeline = InvocationPipeline::new(allowlist, Some(config), Some(skill_repo))
        .with_timeout(Duration::from_millis(50))
        .with_executor(Box::new(SlowExecutor {
            sleep_duration: Duration::from_millis(500),
        }));

    let request = SkillInvocationRequest {
        skill_name: "summarize".to_string(),
        agent_id: "agent-1".to_string(),
        invocation_id: "inv-no-pending".to_string(),
        parameters: HashMap::new(),
        timestamp_ms: 1710000000000,
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel(10);
    pipeline.process(request, tx).await.unwrap();

    // There must be at least one Completed event
    let mut has_completed = false;
    while let Ok(event) = rx.try_recv() {
        if matches!(event, SkillExecutorEvent::Completed { .. }) {
            has_completed = true;
        }
    }
    assert!(
        has_completed,
        "Must have a Completed event — no invocation left pending"
    );
}

// ── AC #2 (5-4): Executor panic produces SKILL_EXECUTION_FAILED ────

/// Given a skill raises an unhandled exception (panic) during execution,
/// When the exception propagates,
/// Then a SkillResult with success: false and error_code: SKILL_EXECUTION_FAILED is published.
#[tokio::test]
async fn test_executor_panic_produces_execution_failed_result() {
    let (_tmp, config, skill_repo) = create_test_git_repo_with_skill();
    let allowlist = SkillAllowlist::new(vec!["summarize".to_string()]);

    let mut pipeline = InvocationPipeline::new(allowlist, Some(config), Some(skill_repo))
        .with_executor(Box::new(PanickingExecutor));

    let request = SkillInvocationRequest {
        skill_name: "summarize".to_string(),
        agent_id: "agent-1".to_string(),
        invocation_id: "inv-panic".to_string(),
        parameters: HashMap::new(),
        timestamp_ms: 1710000000000,
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel(10);
    pipeline.process(request, tx).await.unwrap();

    // Should get exactly one Completed event with SKILL_EXECUTION_FAILED
    let event = rx.recv().await.expect("should receive error event");
    match event {
        SkillExecutorEvent::Completed {
            invocation_id,
            outcome,
        } => {
            assert_eq!(invocation_id, "inv-panic");
            match outcome {
                SkillInvocationOutcome::Error {
                    error_code,
                    error_message,
                } => {
                    assert_eq!(
                        error_code, "SKILL_EXECUTION_FAILED",
                        "Error code must be SKILL_EXECUTION_FAILED"
                    );
                    assert!(
                        error_message.contains("panicked"),
                        "Error message should mention panic: {error_message}"
                    );
                    assert!(
                        error_message.contains("simulated crash"),
                        "Error message should contain panic payload: {error_message}"
                    );
                }
                _ => panic!("Expected Error outcome"),
            }
        }
        _ => panic!("Expected Completed event"),
    }
}

// ── AC #3 (5-4): Wall-clock timeout at executor level ───────────────

/// Given a skill execution hangs silently,
/// When the skill executor's hard wall-clock timeout fires,
/// Then a SkillResult with error_code: SKILL_TIMEOUT is published.
/// The timeout is enforced at the executor/pipeline level.
#[tokio::test]
async fn test_wall_clock_timeout_enforced_at_executor_level() {
    let (_tmp, config, skill_repo) = create_test_git_repo_with_skill();
    let allowlist = SkillAllowlist::new(vec!["summarize".to_string()]);

    // Short timeout (100ms), executor sleeps for 2 seconds
    let mut pipeline = InvocationPipeline::new(allowlist, Some(config), Some(skill_repo))
        .with_timeout(Duration::from_millis(100))
        .with_executor(Box::new(SlowExecutor {
            sleep_duration: Duration::from_secs(2),
        }));

    let request = SkillInvocationRequest {
        skill_name: "summarize".to_string(),
        agent_id: "agent-1".to_string(),
        invocation_id: "inv-wallclock".to_string(),
        parameters: HashMap::new(),
        timestamp_ms: 1710000000000,
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel(10);

    // Time the execution — should complete in ~100ms, not 2s
    let start = std::time::Instant::now();
    pipeline.process(request, tx).await.unwrap();
    let elapsed = start.elapsed();

    // Should complete within 500ms (generous margin for CI)
    assert!(
        elapsed < Duration::from_millis(500),
        "Pipeline should timeout quickly (~100ms), not wait for slow executor. Elapsed: {elapsed:?}"
    );

    // Verify SKILL_TIMEOUT emitted
    let mut events = vec![];
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    let last = events.last().expect("should have events");
    match last {
        SkillExecutorEvent::Completed { outcome, .. } => match outcome {
            SkillInvocationOutcome::Error { error_code, .. } => {
                assert_eq!(error_code, "SKILL_TIMEOUT");
            }
            _ => panic!("Expected Error outcome"),
        },
        _ => panic!("Expected Completed event"),
    }
}

// ── AC #4 (5-4): SkillResult error can be used to build proto ───────

/// Given an agent receives a SkillResult error,
/// Then the builder produces a valid SkillResult proto for SKILL_TIMEOUT.
#[test]
fn test_skill_result_error_builder_skill_timeout() {
    let result = build_skill_result_error(
        "inv-timeout",
        "summarize",
        "SKILL_TIMEOUT",
        "Skill 'summarize' execution timed out after 30s",
    );
    assert_eq!(result.invocation_id, "inv-timeout");
    assert_eq!(result.skill_name, "summarize");
    assert!(!result.success);
    assert_eq!(result.error_code, "SKILL_TIMEOUT");
    assert!(result.error_message.contains("timed out"));
    assert!(result.timestamp_ms > 0);
}

/// Given an agent receives a SkillResult error,
/// Then the builder produces a valid SkillResult proto for SKILL_EXECUTION_FAILED.
#[test]
fn test_skill_result_error_builder_execution_failed() {
    let result = build_skill_result_error(
        "inv-panic",
        "web-search",
        "SKILL_EXECUTION_FAILED",
        "Skill 'web-search' panicked during execution: null pointer",
    );
    assert_eq!(result.invocation_id, "inv-panic");
    assert_eq!(result.skill_name, "web-search");
    assert!(!result.success);
    assert_eq!(result.error_code, "SKILL_EXECUTION_FAILED");
    assert!(result.error_message.contains("panicked"));
    assert!(result.timestamp_ms > 0);
}

// ── Normal execution within timeout succeeds ─────────────────────────

/// Given a skill executes within the timeout window,
/// When the execution completes,
/// Then a normal Success SkillResult is emitted (timeout does not interfere).
#[tokio::test]
async fn test_normal_execution_within_timeout_succeeds() {
    let (_tmp, config, skill_repo) = create_test_git_repo_with_skill();
    let allowlist = SkillAllowlist::new(vec!["summarize".to_string()]);

    // Default timeout (30s) with normal (instant) executor
    let mut pipeline = InvocationPipeline::new(allowlist, Some(config), Some(skill_repo));

    let request = SkillInvocationRequest {
        skill_name: "summarize".to_string(),
        agent_id: "agent-1".to_string(),
        invocation_id: "inv-normal".to_string(),
        parameters: HashMap::new(),
        timestamp_ms: 1710000000000,
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel(10);
    pipeline.process(request, tx).await.unwrap();

    // Skip progress
    let _progress = rx.recv().await.unwrap();

    // Should get Success
    let completed = rx.recv().await.unwrap();
    match completed {
        SkillExecutorEvent::Completed { outcome, .. } => {
            assert!(
                matches!(outcome, SkillInvocationOutcome::Success { .. }),
                "Normal execution should succeed"
            );
        }
        _ => panic!("Expected Completed event"),
    }
}
