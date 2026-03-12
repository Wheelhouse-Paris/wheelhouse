//! Skill executor trait and local implementation.
//!
//! The executor is responsible for running a loaded skill and emitting
//! `SkillProgress` and `SkillResult` events through a channel.

use std::future::Future;
use std::pin::Pin;

use tokio::sync::mpsc;

use crate::invocation::{SkillInvocationOutcome, SkillInvocationRequest};
use crate::repository::LoadedSkill;

/// Events emitted by a skill executor during execution.
#[derive(Debug, Clone)]
pub enum SkillExecutorEvent {
    /// An intermediate progress update (CM-06).
    ProgressUpdate {
        /// The invocation this progress belongs to.
        invocation_id: String,
        /// Progress percentage (0.0 to 1.0).
        progress_percent: f32,
        /// Human-readable status message.
        status_message: String,
    },
    /// The skill execution has completed (terminal event).
    Completed {
        /// The invocation that completed.
        invocation_id: String,
        /// The outcome of the execution.
        outcome: SkillInvocationOutcome,
    },
}

/// Trait for skill executors.
///
/// Implementors run a loaded skill and emit events through a channel.
/// The trait is object-safe and supports async execution.
pub trait SkillExecutor: Send + Sync {
    /// Execute a loaded skill, emitting events through `tx`.
    fn execute<'a>(
        &'a self,
        request: &'a SkillInvocationRequest,
        skill: &'a LoadedSkill,
        tx: &'a mpsc::Sender<SkillExecutorEvent>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

/// A local skill executor that runs skills in-process.
///
/// In MVP, "execution" is a placeholder that concatenates skill step
/// contents as output. Real execution (LLM calls, tool use) is out of scope.
pub struct LocalSkillExecutor;

impl SkillExecutor for LocalSkillExecutor {
    fn execute<'a>(
        &'a self,
        request: &'a SkillInvocationRequest,
        skill: &'a LoadedSkill,
        tx: &'a mpsc::Sender<SkillExecutorEvent>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // CM-06: Emit progress "started" event
            let _ = tx
                .send(SkillExecutorEvent::ProgressUpdate {
                    invocation_id: request.invocation_id.clone(),
                    progress_percent: 0.0,
                    status_message: format!(
                        "Skill '{}' execution started",
                        request.skill_name
                    ),
                })
                .await;

            // MVP placeholder: concatenate step contents as output
            let output = skill
                .steps
                .iter()
                .map(|s| s.content.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");

            // Emit completion event
            let _ = tx
                .send(SkillExecutorEvent::Completed {
                    invocation_id: request.invocation_id.clone(),
                    outcome: SkillInvocationOutcome::Success { output },
                })
                .await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directory::SkillStep;
    use crate::manifest::{SkillManifest, SkillManifestFrontMatter};
    use std::collections::HashMap;

    fn test_skill() -> LoadedSkill {
        LoadedSkill {
            dir_name: "test-skill".into(),
            manifest: SkillManifest {
                front_matter: SkillManifestFrontMatter {
                    name: "test-skill".into(),
                    version: "1.0.0".into(),
                    description: None,
                    inputs: vec![],
                    outputs: vec![],
                    steps: vec!["steps/01-do.md".into()],
                },
                body: String::new(),
            },
            steps: vec![
                SkillStep {
                    filename: "01-do.md".into(),
                    content: "Step 1 content".into(),
                },
                SkillStep {
                    filename: "02-finish.md".into(),
                    content: "Step 2 content".into(),
                },
            ],
        }
    }

    fn test_request() -> SkillInvocationRequest {
        SkillInvocationRequest {
            skill_name: "test-skill".into(),
            agent_id: "agent-1".into(),
            invocation_id: "inv-test".into(),
            parameters: HashMap::new(),
            timestamp_ms: 1710000000000,
        }
    }

    #[tokio::test]
    async fn executor_emits_progress_then_completed() {
        let executor = LocalSkillExecutor;
        let request = test_request();
        let skill = test_skill();
        let (tx, mut rx) = mpsc::channel(10);

        executor.execute(&request, &skill, &tx).await;

        // First: progress
        let first = rx.recv().await.unwrap();
        match first {
            SkillExecutorEvent::ProgressUpdate {
                invocation_id,
                progress_percent,
                ..
            } => {
                assert_eq!(invocation_id, "inv-test");
                assert!((progress_percent - 0.0).abs() < f32::EPSILON);
            }
            _ => panic!("expected ProgressUpdate, got {first:?}"),
        }

        // Second: completed with success
        let second = rx.recv().await.unwrap();
        match second {
            SkillExecutorEvent::Completed {
                invocation_id,
                outcome,
            } => {
                assert_eq!(invocation_id, "inv-test");
                match outcome {
                    SkillInvocationOutcome::Success { output } => {
                        assert!(output.contains("Step 1 content"));
                        assert!(output.contains("Step 2 content"));
                    }
                    _ => panic!("expected Success outcome"),
                }
            }
            _ => panic!("expected Completed, got {second:?}"),
        }
    }

    #[tokio::test]
    async fn executor_output_concatenates_steps() {
        let executor = LocalSkillExecutor;
        let request = test_request();
        let skill = test_skill();
        let (tx, mut rx) = mpsc::channel(10);

        executor.execute(&request, &skill, &tx).await;

        // Skip progress
        let _ = rx.recv().await.unwrap();

        let completed = rx.recv().await.unwrap();
        match completed {
            SkillExecutorEvent::Completed { outcome, .. } => match outcome {
                SkillInvocationOutcome::Success { output } => {
                    assert_eq!(output, "Step 1 content\n\nStep 2 content");
                }
                _ => panic!("expected Success"),
            },
            _ => panic!("expected Completed"),
        }
    }

    #[tokio::test]
    async fn executor_handles_single_step_skill() {
        let executor = LocalSkillExecutor;
        let request = test_request();
        let skill = LoadedSkill {
            dir_name: "single".into(),
            manifest: SkillManifest {
                front_matter: SkillManifestFrontMatter {
                    name: "single".into(),
                    version: "1.0.0".into(),
                    description: None,
                    inputs: vec![],
                    outputs: vec![],
                    steps: vec!["steps/01-only.md".into()],
                },
                body: String::new(),
            },
            steps: vec![SkillStep {
                filename: "01-only.md".into(),
                content: "Only step".into(),
            }],
        };
        let (tx, mut rx) = mpsc::channel(10);

        executor.execute(&request, &skill, &tx).await;

        let _ = rx.recv().await.unwrap(); // skip progress
        let completed = rx.recv().await.unwrap();
        match completed {
            SkillExecutorEvent::Completed { outcome, .. } => match outcome {
                SkillInvocationOutcome::Success { output } => {
                    assert_eq!(output, "Only step");
                }
                _ => panic!("expected Success"),
            },
            _ => panic!("expected Completed"),
        }
    }
}
