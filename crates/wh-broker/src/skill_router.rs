//! Skill router — routes `SkillInvocation` messages to the skill execution pipeline.
//!
//! When the broker detects a `SkillInvocation` in the routing loop (by inspecting
//! `StreamEnvelope.type_url`), the `SkillRouter` looks up the agent's pipeline
//! and executes the skill, producing `SkillResult` and `SkillProgress` messages
//! to publish back to the stream.
//!
//! Architecture reference: `Agent -> SkillInvocation -> Broker -> skill executor`
//! (architecture.md, lines 839-845)

use std::collections::HashMap;
use std::path::PathBuf;

use prost::Message;
use tokio::sync::{mpsc, Mutex};

use wh_proto::StreamEnvelope;
use wh_skill::executor::SkillExecutorEvent;
use wh_skill::invocation::{
    build_skill_result_error, build_skill_result_success, build_skill_progress,
    SkillInvocationOutcome, SkillInvocationRequest,
};
use wh_skill::{InvocationPipeline, SkillAllowlist, SkillRepository};

/// Type URL for SkillInvocation messages.
pub const TYPE_URL_SKILL_INVOCATION: &str = "wheelhouse.v1.SkillInvocation";
/// Type URL for SkillResult messages.
pub const TYPE_URL_SKILL_RESULT: &str = "wheelhouse.v1.SkillResult";
/// Type URL for SkillProgress messages.
pub const TYPE_URL_SKILL_PROGRESS: &str = "wheelhouse.v1.SkillProgress";
/// Publisher ID for skill results published by the broker.
pub const BROKER_SKILL_PUBLISHER_ID: &str = "broker-skill-executor";

/// A response message produced by skill execution.
///
/// Contains the type_url and encoded protobuf bytes ready to be wrapped
/// in a `StreamEnvelope` by the caller.
#[derive(Debug)]
pub struct SkillResponse {
    /// The protobuf type URL (e.g., `wheelhouse.v1.SkillResult`).
    pub type_url: String,
    /// Encoded protobuf bytes of the response message.
    pub payload: Vec<u8>,
}

/// Routes `SkillInvocation` messages to per-agent execution pipelines.
///
/// Each registered agent has its own `InvocationPipeline` behind a `Mutex`
/// to serialize access (pipeline cache updates require `&mut self`).
/// Agents don't contend with each other since each has its own lock.
pub struct SkillRouter {
    /// Map of agent_id -> pipeline. Each pipeline is behind its own Mutex
    /// because `InvocationPipeline::process()` takes `&mut self`.
    pipelines: HashMap<String, Mutex<InvocationPipeline>>,
}

impl SkillRouter {
    /// Create a new empty skill router with no agents registered.
    pub fn new() -> Self {
        SkillRouter {
            pipelines: HashMap::new(),
        }
    }

    /// Register an agent's skill pipeline.
    ///
    /// Creates an `InvocationPipeline` with:
    /// - An allowlist from the provided skill names (FM-05)
    /// - A `SkillRepository` opened at `skills_repo` path (if provided)
    /// - A `SkillsConfig` built from the skill references
    ///
    /// If `skills_repo` path cannot be opened, the pipeline is still created
    /// but skill fetching will fail with `SKILL_FETCH_FAILED` at invocation time.
    pub fn register_agent(
        &mut self,
        agent_id: &str,
        skill_names: Vec<String>,
        skills_repo_path: Option<&str>,
        skill_refs: Vec<wh_skill::config::SkillRef>,
    ) {
        let allowlist = SkillAllowlist::new(skill_names);

        let config = if !skill_refs.is_empty() {
            Some(wh_skill::config::SkillsConfig {
                skills_repo: skills_repo_path
                    .map(PathBuf::from)
                    .unwrap_or_default(),
                skills: skill_refs,
            })
        } else {
            None
        };

        let repo = skills_repo_path.and_then(|path| {
            match SkillRepository::open(std::path::Path::new(path)) {
                Ok(r) => Some(r),
                Err(e) => {
                    tracing::error!(
                        agent_id = agent_id,
                        path = path,
                        error = %e,
                        "failed to open skills repository — skill fetching will fail"
                    );
                    None
                }
            }
        });

        let pipeline = InvocationPipeline::new(allowlist, config, repo);

        self.pipelines
            .insert(agent_id.to_string(), Mutex::new(pipeline));

        tracing::info!(
            agent_id = agent_id,
            skills_repo = ?skills_repo_path,
            "registered skill pipeline for agent"
        );
    }

    /// Check if any agents have registered skill pipelines.
    pub fn is_empty(&self) -> bool {
        self.pipelines.is_empty()
    }

    /// Handle a `SkillInvocation`, returning response messages to publish.
    ///
    /// Looks up the pipeline by `invocation.agent_id`. If no pipeline exists,
    /// returns a `SkillResult` error with `SKILL_NOT_PERMITTED`.
    ///
    /// Every invocation produces exactly one terminal `SkillResult` (Story 5-4).
    /// May also produce intermediate `SkillProgress` messages.
    pub async fn handle_invocation(
        &self,
        invocation: SkillInvocationRequest,
    ) -> Vec<SkillResponse> {
        let pipeline_mutex = match self.pipelines.get(&invocation.agent_id) {
            Some(p) => p,
            None => {
                // No pipeline for this agent — check if there's a "default" pipeline
                match self.pipelines.get("default") {
                    Some(p) => p,
                    None => {
                        tracing::error!(
                            agent_id = %invocation.agent_id,
                            skill_name = %invocation.skill_name,
                            invocation_id = %invocation.invocation_id,
                            "no skill pipeline registered for agent (FR41)"
                        );
                        let result = build_skill_result_error(
                            &invocation.invocation_id,
                            &invocation.skill_name,
                            "SKILL_NOT_PERMITTED",
                            &format!(
                                "No skill pipeline configured for agent '{}'",
                                invocation.agent_id
                            ),
                        );
                        return vec![SkillResponse {
                            type_url: TYPE_URL_SKILL_RESULT.to_string(),
                            payload: result.encode_to_vec(),
                        }];
                    }
                }
            }
        };

        let (tx, mut rx) = mpsc::channel::<SkillExecutorEvent>(16);

        // Execute the skill through the pipeline
        {
            let mut pipeline = pipeline_mutex.lock().await;
            if let Err(e) = pipeline.process(invocation.clone(), tx).await {
                tracing::error!(
                    agent_id = %invocation.agent_id,
                    skill_name = %invocation.skill_name,
                    invocation_id = %invocation.invocation_id,
                    error = %e,
                    "skill pipeline error (FR41)"
                );
                // Pipeline errors should still produce a result via the channel,
                // but if something truly unexpected happens, emit one here.
                let result = build_skill_result_error(
                    &invocation.invocation_id,
                    &invocation.skill_name,
                    "SKILL_EXECUTION_FAILED",
                    &format!("Pipeline error: {e}"),
                );
                return vec![SkillResponse {
                    type_url: TYPE_URL_SKILL_RESULT.to_string(),
                    payload: result.encode_to_vec(),
                }];
            }
        }

        // Collect all events from the channel.
        // The tx sender was moved into pipeline.process() and is now dropped,
        // so recv() will return None once all buffered events are consumed.
        let mut responses = Vec::new();
        while let Some(event) = rx.recv().await {
            match event {
                SkillExecutorEvent::ProgressUpdate {
                    invocation_id,
                    progress_percent,
                    status_message,
                } => {
                    let progress = build_skill_progress(
                        &invocation_id,
                        &invocation.skill_name,
                        progress_percent,
                        &status_message,
                    );
                    responses.push(SkillResponse {
                        type_url: TYPE_URL_SKILL_PROGRESS.to_string(),
                        payload: progress.encode_to_vec(),
                    });
                }
                SkillExecutorEvent::Completed {
                    invocation_id,
                    outcome,
                } => {
                    let result_proto = match outcome {
                        SkillInvocationOutcome::Success { output } => {
                            build_skill_result_success(
                                &invocation_id,
                                &invocation.skill_name,
                                &output,
                            )
                        }
                        SkillInvocationOutcome::Error {
                            error_code,
                            error_message,
                        } => {
                            tracing::error!(
                                agent_id = %invocation.agent_id,
                                skill_name = %invocation.skill_name,
                                invocation_id = %invocation_id,
                                error_code = %error_code,
                                error_message = %error_message,
                                "skill execution failed (FR41)"
                            );
                            build_skill_result_error(
                                &invocation_id,
                                &invocation.skill_name,
                                &error_code,
                                &error_message,
                            )
                        }
                    };
                    responses.push(SkillResponse {
                        type_url: TYPE_URL_SKILL_RESULT.to_string(),
                        payload: result_proto.encode_to_vec(),
                    });
                }
            }
        }

        responses
    }
}

/// Build a `StreamEnvelope` for a skill response message.
pub fn build_response_envelope(
    stream_name: &str,
    response: &SkillResponse,
) -> StreamEnvelope {
    StreamEnvelope {
        stream_name: stream_name.to_string(),
        object_id: String::new(),
        type_url: response.type_url.clone(),
        payload: response.payload.clone(),
        publisher_id: BROKER_SKILL_PUBLISHER_ID.to_string(),
        published_at_ms: 0, // broker assigns
        sequence_number: 0, // broker assigns
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_invocation(skill_name: &str, agent_id: &str) -> SkillInvocationRequest {
        SkillInvocationRequest {
            skill_name: skill_name.to_string(),
            agent_id: agent_id.to_string(),
            invocation_id: "inv-test-001".to_string(),
            parameters: HashMap::new(),
            timestamp_ms: 1710000000000,
        }
    }

    #[test]
    fn new_router_is_empty() {
        let router = SkillRouter::new();
        assert!(router.is_empty());
    }

    #[tokio::test]
    async fn unregistered_agent_returns_not_permitted() {
        let router = SkillRouter::new();
        let invocation = test_invocation("summarize", "unknown-agent");

        let responses = router.handle_invocation(invocation).await;
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].type_url, TYPE_URL_SKILL_RESULT);

        // Decode and verify error
        let result = wh_proto::SkillResult::decode(responses[0].payload.as_slice()).unwrap();
        assert!(!result.success);
        assert_eq!(result.error_code, "SKILL_NOT_PERMITTED");
        assert!(result.error_message.contains("unknown-agent"));
    }

    #[tokio::test]
    async fn registered_agent_allowed_skill_produces_result() {
        let mut router = SkillRouter::new();

        // Register with allowlist but no repo — skill will fail at fetch stage
        // but will produce a SkillResult (not crash)
        router.register_agent(
            "agent-1",
            vec!["summarize".to_string()],
            None,
            vec![wh_skill::config::SkillRef {
                name: "summarize".to_string(),
                version: "1.0.0".to_string(),
            }],
        );

        assert!(!router.is_empty());

        let invocation = test_invocation("summarize", "agent-1");
        let responses = router.handle_invocation(invocation).await;

        // Should get exactly one SkillResult (error due to no repo, but still a result)
        assert!(!responses.is_empty());
        let result_response = responses.iter().find(|r| r.type_url == TYPE_URL_SKILL_RESULT);
        assert!(result_response.is_some(), "should produce a SkillResult");

        let result =
            wh_proto::SkillResult::decode(result_response.unwrap().payload.as_slice()).unwrap();
        assert!(!result.success);
        assert_eq!(result.error_code, "SKILL_FETCH_FAILED");
    }

    #[tokio::test]
    async fn registered_agent_disallowed_skill_returns_not_permitted() {
        let mut router = SkillRouter::new();

        router.register_agent(
            "agent-1",
            vec!["summarize".to_string()],
            None,
            vec![wh_skill::config::SkillRef {
                name: "summarize".to_string(),
                version: "1.0.0".to_string(),
            }],
        );

        let invocation = test_invocation("web-search", "agent-1");
        let responses = router.handle_invocation(invocation).await;

        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].type_url, TYPE_URL_SKILL_RESULT);

        let result = wh_proto::SkillResult::decode(responses[0].payload.as_slice()).unwrap();
        assert!(!result.success);
        assert_eq!(result.error_code, "SKILL_NOT_PERMITTED");
    }

    #[tokio::test]
    async fn default_pipeline_used_for_unknown_agent() {
        let mut router = SkillRouter::new();

        // Register a "default" pipeline
        router.register_agent(
            "default",
            vec!["summarize".to_string()],
            None,
            vec![wh_skill::config::SkillRef {
                name: "summarize".to_string(),
                version: "1.0.0".to_string(),
            }],
        );

        // Invoke with an unknown agent — should fall through to "default"
        let invocation = test_invocation("summarize", "any-agent");
        let responses = router.handle_invocation(invocation).await;

        // Should get a result from the default pipeline (SKILL_FETCH_FAILED since no repo)
        assert!(!responses.is_empty());
        let result_response = responses.iter().find(|r| r.type_url == TYPE_URL_SKILL_RESULT);
        assert!(result_response.is_some());

        let result =
            wh_proto::SkillResult::decode(result_response.unwrap().payload.as_slice()).unwrap();
        // It found the pipeline, so error should be about fetching, not permissions
        assert_eq!(result.error_code, "SKILL_FETCH_FAILED");
    }

    #[test]
    fn build_response_envelope_sets_publisher_id() {
        let response = SkillResponse {
            type_url: TYPE_URL_SKILL_RESULT.to_string(),
            payload: vec![1, 2, 3],
        };
        let envelope = build_response_envelope("test-stream", &response);
        assert_eq!(envelope.publisher_id, BROKER_SKILL_PUBLISHER_ID);
        assert_eq!(envelope.stream_name, "test-stream");
        assert_eq!(envelope.type_url, TYPE_URL_SKILL_RESULT);
        assert_eq!(envelope.sequence_number, 0);
        assert_eq!(envelope.published_at_ms, 0);
    }

    #[test]
    fn type_url_constants_match_proto_package() {
        assert!(TYPE_URL_SKILL_INVOCATION.starts_with("wheelhouse.v1."));
        assert!(TYPE_URL_SKILL_RESULT.starts_with("wheelhouse.v1."));
        assert!(TYPE_URL_SKILL_PROGRESS.starts_with("wheelhouse.v1."));
    }
}
