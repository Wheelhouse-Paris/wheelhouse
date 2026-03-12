//! CronSkillHandler — bridges cron events to skill invocations.
//!
//! When a CronEvent arrives for a registered job, this handler constructs a
//! SkillInvocation and emits a ChainEvent for the orchestrator.

use async_trait::async_trait;
use tokio::sync::mpsc;

use super::chain::ChainEvent;
use super::handler::{CronEventHandler, CronHandlerError, HandlerOutcome};
use super::proto_bridge;
use super::CronEventMessage;

/// Bridges cron events to skill invocations.
/// Registered with the CronEventDispatcher for a specific job name.
pub struct CronSkillHandler {
    /// The skill to invoke when this cron job fires.
    pub skill_name: String,
    /// The agent ID that owns this cron -> skill binding.
    pub agent_id: String,
    /// Channel to send chain events to the orchestrator.
    pub event_sender: mpsc::Sender<ChainEvent>,
}

#[async_trait]
impl CronEventHandler for CronSkillHandler {
    #[tracing::instrument(skip_all, fields(job_name = %event.job_name, skill = %self.skill_name))]
    async fn handle(&self, event: CronEventMessage) -> Result<HandlerOutcome, CronHandlerError> {
        // Build the SkillInvocation proto from the cron event
        let invocation = proto_bridge::build_skill_invocation_from_cron(
            &event,
            &self.skill_name,
            &self.agent_id,
        );

        let invocation_id = invocation.invocation_id.clone();
        let skill_name = invocation.skill_name.clone();
        let timestamp_ms = invocation.timestamp_ms;

        // Emit chain event for the orchestrator
        let chain_event = ChainEvent::SkillInvocationPublished {
            invocation_id,
            skill_name: skill_name.clone(),
            timestamp_ms,
        };

        self.event_sender.send(chain_event).await.map_err(|_| {
            CronHandlerError::ExecutionFailed {
                job_name: event.job_name.clone(),
                source: "chain event channel closed".into(),
            }
        })?;

        Ok(HandlerOutcome::Completed {
            message: format!("skill invocation published for {}", skill_name),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn handler_sends_skill_invocation_event() {
        let (tx, mut rx) = mpsc::channel(10);
        let handler = CronSkillHandler {
            skill_name: "echo".into(),
            agent_id: "agent-1".into(),
            event_sender: tx,
        };

        let event = CronEventMessage {
            job_name: "echo-cron".into(),
            action: "event".into(),
            schedule: "* * * * *".into(),
            triggered_at: prost_types::Timestamp {
                seconds: 0,
                nanos: 0,
            },
            payload: HashMap::new(),
            target_stream: "test-stream".into(),
        };

        let result = handler.handle(event).await;
        assert!(result.is_ok());

        let chain_event = rx.recv().await.unwrap();
        match chain_event {
            ChainEvent::SkillInvocationPublished {
                invocation_id,
                skill_name,
                timestamp_ms,
            } => {
                assert!(!invocation_id.is_empty());
                assert_eq!(skill_name, "echo");
                assert!(timestamp_ms > 0);
            }
            _ => panic!("expected SkillInvocationPublished event"),
        }
    }

    #[tokio::test]
    async fn handler_maps_cron_payload_to_invocation_parameters() {
        let (tx, _rx) = mpsc::channel(10);
        let handler = CronSkillHandler {
            skill_name: "echo".into(),
            agent_id: "agent-1".into(),
            event_sender: tx,
        };

        let event = CronEventMessage {
            job_name: "echo-cron".into(),
            action: "event".into(),
            schedule: "* * * * *".into(),
            triggered_at: prost_types::Timestamp {
                seconds: 0,
                nanos: 0,
            },
            payload: [("input".into(), "hello world".into())]
                .into_iter()
                .collect(),
            target_stream: "test-stream".into(),
        };

        // The handler internally calls build_skill_invocation_from_cron which maps payload.
        // We verify the handler succeeds with the payload — the proto_bridge tests
        // verify the actual field mapping.
        let result = handler.handle(event).await;
        assert!(result.is_ok());
    }
}
