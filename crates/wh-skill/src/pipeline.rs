//! Skill invocation pipeline: allowlist validation + lazy loading + timeout + execution.
//!
//! The pipeline validates an invocation against the allowlist (FM-05),
//! resolves the skill version from config, checks the cache for a previously
//! loaded skill, and if not cached, fetches from git and populates the cache.
//! Finally, executes the skill via a [`SkillExecutor`] with a wall-clock
//! timeout (Story 5-4) and panic safety.

use std::panic::AssertUnwindSafe;
use std::time::Duration;

use futures::FutureExt;
use tokio::sync::mpsc;

use crate::allowlist::SkillAllowlist;
use crate::cache::SkillCache;
use crate::config::SkillsConfig;
use crate::error::SkillError;
use crate::executor::{LocalSkillExecutor, SkillExecutor, SkillExecutorEvent};
use crate::invocation::{SkillInvocationOutcome, SkillInvocationRequest};
use crate::repository::SkillRepository;

/// Default skill execution timeout: 30 seconds (aligned with CM-04 git timeout).
pub const DEFAULT_SKILL_TIMEOUT: Duration = Duration::from_secs(30);

/// The invocation pipeline: allowlist check -> version resolution -> cache/fetch -> execute.
///
/// Every invocation produces exactly one terminal `SkillResult` — success or error.
/// No invocation is ever left in a pending state (Story 5-4 error contract).
pub struct InvocationPipeline {
    allowlist: SkillAllowlist,
    /// Skills config for version resolution. None means no version resolution is possible.
    config: Option<SkillsConfig>,
    /// Git-based skill repository. None means skills cannot be fetched.
    repo: Option<SkillRepository>,
    /// Session-scoped skill cache for lazy loading.
    cache: SkillCache,
    /// Wall-clock timeout for skill execution (Story 5-4).
    timeout: Duration,
    /// The executor to use for running skills.
    executor: Box<dyn SkillExecutor>,
}

impl InvocationPipeline {
    /// Create a new pipeline with an allowlist, optional skill repository, and a cache.
    ///
    /// Uses `LocalSkillExecutor` as the default executor.
    /// No skills are loaded at construction time — the cache starts empty and is
    /// populated lazily on first invocation of each skill (AC1).
    ///
    /// If `config` is None, version resolution will fail for any invocation.
    /// If `repo` is None, skill fetching will fail for any invocation.
    ///
    /// Default timeout is 30 seconds (see [`DEFAULT_SKILL_TIMEOUT`]).
    pub fn new(
        allowlist: SkillAllowlist,
        config: Option<SkillsConfig>,
        repo: Option<SkillRepository>,
    ) -> Self {
        InvocationPipeline {
            allowlist,
            config,
            repo,
            cache: SkillCache::new(),
            timeout: DEFAULT_SKILL_TIMEOUT,
            executor: Box::new(LocalSkillExecutor),
        }
    }

    /// Set a custom execution timeout for this pipeline.
    ///
    /// The timeout applies to the skill executor call (Step 4). If execution
    /// exceeds this duration, a `SKILL_TIMEOUT` error is emitted.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set a custom executor for this pipeline.
    ///
    /// Primarily used for testing (e.g., injecting a slow or panicking executor).
    pub fn with_executor(mut self, executor: Box<dyn SkillExecutor>) -> Self {
        self.executor = executor;
        self
    }

    /// Return the configured timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Return a reference to the skill cache (for testing/inspection).
    pub fn cache(&self) -> &SkillCache {
        &self.cache
    }

    /// Process a skill invocation request.
    ///
    /// Steps:
    /// 1. Validate the skill is in the allowlist (FM-05)
    /// 2. Resolve the skill version from config
    /// 3. Check cache; if miss, fetch from git and populate cache
    /// 4. Execute the skill with timeout + panic safety, emit progress/result events
    ///
    /// On any failure, emits a `Completed` event with an error outcome.
    /// Every invocation always produces exactly one terminal event (Story 5-4).
    pub async fn process(
        &mut self,
        request: SkillInvocationRequest,
        tx: mpsc::Sender<SkillExecutorEvent>,
    ) -> Result<(), SkillError> {
        // Step 1: Allowlist validation (FM-05)
        if self.allowlist.validate(&request.skill_name, &request.agent_id).is_err() {
            let _ = tx
                .send(SkillExecutorEvent::Completed {
                    invocation_id: request.invocation_id.clone(),
                    outcome: SkillInvocationOutcome::Error {
                        error_code: "SKILL_NOT_PERMITTED".into(),
                        error_message: format!(
                            "Skill '{}' is not in agent '{}' allowlist",
                            request.skill_name, request.agent_id
                        ),
                    },
                })
                .await;
            return Ok(());
        }

        // Step 2: Resolve version from config
        let version = match &self.config {
            Some(config) => {
                match config.skills.iter().find(|s| s.name == request.skill_name) {
                    Some(skill_ref) => skill_ref.version.clone(),
                    None => {
                        let _ = tx
                            .send(SkillExecutorEvent::Completed {
                                invocation_id: request.invocation_id.clone(),
                                outcome: SkillInvocationOutcome::Error {
                                    error_code: "SKILL_LOAD_FAILED".into(),
                                    error_message: format!(
                                        "Skill '{}' not found in skills config",
                                        request.skill_name
                                    ),
                                },
                            })
                            .await;
                        return Ok(());
                    }
                }
            }
            None => {
                let _ = tx
                    .send(SkillExecutorEvent::Completed {
                        invocation_id: request.invocation_id.clone(),
                        outcome: SkillInvocationOutcome::Error {
                            error_code: "SKILL_LOAD_FAILED".into(),
                            error_message: "No skills config available for version resolution"
                                .into(),
                        },
                    })
                    .await;
                return Ok(());
            }
        };

        // Step 3: Resolve OID and check cache; fetch from git on miss
        let repo = match &self.repo {
            Some(r) => r,
            None => {
                let _ = tx
                    .send(SkillExecutorEvent::Completed {
                        invocation_id: request.invocation_id.clone(),
                        outcome: SkillInvocationOutcome::Error {
                            error_code: "SKILL_FETCH_FAILED".into(),
                            error_message: format!(
                                "No skill repository available to fetch skill '{}'",
                                request.skill_name
                            ),
                        },
                    })
                    .await;
                return Ok(());
            }
        };

        let oid = match repo.resolve_version(&version) {
            Ok(oid) => oid,
            Err(_) => {
                let _ = tx
                    .send(SkillExecutorEvent::Completed {
                        invocation_id: request.invocation_id.clone(),
                        outcome: SkillInvocationOutcome::Error {
                            error_code: "SKILL_FETCH_FAILED".into(),
                            error_message: format!(
                                "Failed to resolve version '{}' for skill '{}'",
                                version, request.skill_name
                            ),
                        },
                    })
                    .await;
                return Ok(());
            }
        };

        // Check cache before git fetch
        if !self.cache.contains(&request.skill_name, oid) {
            // Cache miss — fetch from git
            match repo.load_skill_at(&request.skill_name, oid) {
                Ok(skill) => {
                    self.cache.insert(&request.skill_name, oid, skill);
                }
                Err(_) => {
                    let _ = tx
                        .send(SkillExecutorEvent::Completed {
                            invocation_id: request.invocation_id.clone(),
                            outcome: SkillInvocationOutcome::Error {
                                error_code: "SKILL_FETCH_FAILED".into(),
                                error_message: format!(
                                    "Failed to fetch skill '{}' at version '{}'",
                                    request.skill_name, version
                                ),
                            },
                        })
                        .await;
                    return Ok(());
                }
            }
        }

        // Step 4: Execute from cache with timeout + panic safety (Story 5-4)
        let loaded_skill = self
            .cache
            .get(&request.skill_name, oid)
            .expect("skill was just inserted into cache")
            .clone();

        let timeout_duration = self.timeout;
        let skill_name = request.skill_name.clone();
        let invocation_id = request.invocation_id.clone();

        // Wrap executor in catch_unwind for panic safety, then wrap in timeout
        let executor_future =
            AssertUnwindSafe(self.executor.execute(&request, &loaded_skill, &tx))
                .catch_unwind();

        match tokio::time::timeout(timeout_duration, executor_future).await {
            Ok(Ok(())) => {
                // Executor completed successfully — events already emitted by executor
            }
            Ok(Err(panic_info)) => {
                // Executor panicked — emit SKILL_EXECUTION_FAILED
                let panic_message = if let Some(s) = panic_info.downcast_ref::<&str>() {
                    (*s).to_string()
                } else if let Some(s) = panic_info.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                let _ = tx
                    .send(SkillExecutorEvent::Completed {
                        invocation_id,
                        outcome: SkillInvocationOutcome::Error {
                            error_code: "SKILL_EXECUTION_FAILED".into(),
                            error_message: format!(
                                "Skill '{}' panicked during execution: {}",
                                skill_name, panic_message
                            ),
                        },
                    })
                    .await;
            }
            Err(_elapsed) => {
                // Timeout — emit SKILL_TIMEOUT
                let _ = tx
                    .send(SkillExecutorEvent::Completed {
                        invocation_id,
                        outcome: SkillInvocationOutcome::Error {
                            error_code: "SKILL_TIMEOUT".into(),
                            error_message: format!(
                                "Skill '{}' execution timed out after {:.1}s",
                                skill_name,
                                timeout_duration.as_secs_f64()
                            ),
                        },
                    })
                    .await;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn disallowed_request() -> SkillInvocationRequest {
        SkillInvocationRequest {
            skill_name: "forbidden-skill".into(),
            agent_id: "agent-1".into(),
            invocation_id: "inv-reject".into(),
            parameters: HashMap::new(),
            timestamp_ms: 1710000000000,
        }
    }

    #[tokio::test]
    async fn rejects_disallowed_skill() {
        let allowlist = SkillAllowlist::new(vec!["summarize".into()]);
        let mut pipeline = InvocationPipeline::new(allowlist, None, None);
        let (tx, mut rx) = mpsc::channel(10);

        pipeline.process(disallowed_request(), tx).await.unwrap();

        let event = rx.recv().await.unwrap();
        match event {
            SkillExecutorEvent::Completed {
                invocation_id,
                outcome,
            } => {
                assert_eq!(invocation_id, "inv-reject");
                match outcome {
                    SkillInvocationOutcome::Error {
                        error_code,
                        error_message,
                    } => {
                        assert_eq!(error_code, "SKILL_NOT_PERMITTED");
                        assert!(error_message.contains("forbidden-skill"));
                    }
                    _ => panic!("expected Error outcome"),
                }
            }
            _ => panic!("expected Completed event"),
        }
    }

    #[tokio::test]
    async fn allowed_skill_without_repo_gives_fetch_error() {
        let allowlist = SkillAllowlist::new(vec!["summarize".into()]);
        let config = SkillsConfig {
            skills_repo: "/path".into(),
            skills: vec![crate::config::SkillRef {
                name: "summarize".into(),
                version: "1.0.0".into(),
            }],
        };
        let mut pipeline = InvocationPipeline::new(allowlist, Some(config), None);
        let request = SkillInvocationRequest {
            skill_name: "summarize".into(),
            agent_id: "agent-1".into(),
            invocation_id: "inv-no-repo".into(),
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
                _ => panic!("expected Error"),
            },
            _ => panic!("expected Completed"),
        }
    }

    #[tokio::test]
    async fn allowed_skill_not_in_config_gives_load_error() {
        let allowlist = SkillAllowlist::new(vec!["summarize".into()]);
        let config = SkillsConfig {
            skills_repo: "/path".into(),
            skills: vec![], // empty config — skill not found
        };
        let mut pipeline = InvocationPipeline::new(allowlist, Some(config), None);
        let request = SkillInvocationRequest {
            skill_name: "summarize".into(),
            agent_id: "agent-1".into(),
            invocation_id: "inv-no-config".into(),
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
                    assert_eq!(error_code, "SKILL_LOAD_FAILED");
                    assert!(error_message.contains("not found in skills config"));
                }
                _ => panic!("expected Error"),
            },
            _ => panic!("expected Completed"),
        }
    }

    #[tokio::test]
    async fn pipeline_cache_starts_empty() {
        let allowlist = SkillAllowlist::new(vec!["summarize".into()]);
        let pipeline = InvocationPipeline::new(allowlist, None, None);
        assert!(pipeline.cache().is_empty());
    }

    // ── Story 5-4: Timeout and error contract tests ──────────────────

    #[test]
    fn default_timeout_is_30_seconds() {
        assert_eq!(DEFAULT_SKILL_TIMEOUT, Duration::from_secs(30));
    }

    #[test]
    fn pipeline_uses_default_timeout() {
        let allowlist = SkillAllowlist::new(vec![]);
        let pipeline = InvocationPipeline::new(allowlist, None, None);
        assert_eq!(pipeline.timeout(), DEFAULT_SKILL_TIMEOUT);
    }

    #[test]
    fn pipeline_with_custom_timeout() {
        let allowlist = SkillAllowlist::new(vec![]);
        let pipeline = InvocationPipeline::new(allowlist, None, None)
            .with_timeout(Duration::from_millis(500));
        assert_eq!(pipeline.timeout(), Duration::from_millis(500));
    }
}
