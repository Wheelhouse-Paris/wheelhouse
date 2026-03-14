//! CronSkillChain — end-to-end orchestrator for cron -> skill invocation chain.
//!
//! Processes a CronEventMessage through the full chain:
//! CronEvent -> SkillInvocation -> SkillProgress -> SkillResult -> TextMessage

use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::{error, info};

use super::chain::{ChainError, ChainEvent, ChainOutcome, NotificationType, SurfaceNotification};
use super::dispatcher::CronEventDispatcher;
use super::proto_bridge;
use super::CronEventMessage;

/// Orchestrates the cron -> skill invocation -> result -> summary chain.
pub struct CronSkillChain {
    dispatcher: CronEventDispatcher,
    event_receiver: mpsc::Receiver<ChainEvent>,
    chain_log: Vec<ChainEvent>,
    notification_sender: Option<mpsc::Sender<SurfaceNotification>>,
    /// Agent ID for TextMessage authorship.
    agent_id: String,
}

impl CronSkillChain {
    /// Create a new chain orchestrator.
    pub fn new(
        dispatcher: CronEventDispatcher,
        event_receiver: mpsc::Receiver<ChainEvent>,
        agent_id: &str,
    ) -> Self {
        Self {
            dispatcher,
            event_receiver,
            chain_log: Vec::new(),
            notification_sender: None,
            agent_id: agent_id.to_string(),
        }
    }

    /// Set the notification sender for Surface notifications (AC#3).
    pub fn set_notification_sender(&mut self, sender: mpsc::Sender<SurfaceNotification>) {
        self.notification_sender = Some(sender);
    }

    /// Process a cron event through the full chain.
    ///
    /// Steps:
    /// 1. Log CronEventReceived
    /// 2. Dispatch to CronSkillHandler
    /// 3. Await SkillInvocationPublished from handler
    /// 4. Emit SkillProgress "started" (CM-06)
    /// 5. Simulate skill execution -> SkillResult
    /// 6. Construct TextMessage summary
    /// 7. On failure: send SurfaceNotification
    #[tracing::instrument(skip_all, fields(job_name = %event.job_name))]
    pub async fn process_cron_event(
        &mut self,
        event: CronEventMessage,
    ) -> Result<ChainOutcome, ChainError> {
        self.chain_log.clear();
        let job_name = event.job_name.clone();

        // Step 1: Log CronEventReceived
        let cron_received = ChainEvent::CronEventReceived {
            job_name: job_name.clone(),
            timestamp_ms: proto_bridge::now_ms(),
        };
        self.chain_log.push(cron_received);

        // Step 2: Dispatch to handler
        let handle = self
            .dispatcher
            .dispatch(event)
            .ok_or_else(|| ChainError::DispatchFailed {
                job_name: job_name.clone(),
                reason: "no handler registered".into(),
            })?;

        // Step 3: Await handler completion and SkillInvocationPublished event
        // Wait for the spawned task to complete
        handle.await.map_err(|e| ChainError::InvocationFailed {
            job_name: job_name.clone(),
            reason: format!("handler task panicked: {e}"),
        })?;

        // Receive the SkillInvocationPublished event from the handler
        let invocation_event =
            self.event_receiver
                .recv()
                .await
                .ok_or_else(|| ChainError::ChannelClosed {
                    job_name: job_name.clone(),
                })?;

        let (invocation_id, skill_name) = match &invocation_event {
            ChainEvent::SkillInvocationPublished {
                invocation_id,
                skill_name,
                ..
            } => (invocation_id.clone(), skill_name.clone()),
            _ => {
                return Err(ChainError::InvocationFailed {
                    job_name: job_name.clone(),
                    reason: "unexpected event type from handler".into(),
                });
            }
        };
        self.chain_log.push(invocation_event);

        // Step 4: Emit SkillProgress "started" (CM-06 compliance)
        let progress_event = ChainEvent::SkillProgressPublished {
            invocation_id: invocation_id.clone(),
            percent: 0,
            message: "started".into(),
            timestamp_ms: proto_bridge::now_ms(),
        };
        self.chain_log.push(progress_event);

        // Step 5: Simulate skill execution (MVP: immediate result)
        // In a full implementation, this would route through the skill executor.
        // For the integration gate, we simulate success/failure based on skill_name.
        let (success, output_or_error) = self.simulate_skill_execution(&skill_name);

        let result_event = ChainEvent::SkillResultReceived {
            invocation_id: invocation_id.clone(),
            success,
            output_or_error: output_or_error.clone(),
            timestamp_ms: proto_bridge::now_ms(),
        };
        self.chain_log.push(result_event);

        // Step 6/7: Construct TextMessage and handle failure
        if success {
            let summary = format!(
                "[CRON CHAIN OK] job={job_name} skill={skill_name} output={output_or_error}"
            );
            let _ = proto_bridge::build_text_message(&summary, &self.agent_id);

            let text_event = ChainEvent::TextMessagePublished {
                content: summary.clone(),
                timestamp_ms: proto_bridge::now_ms(),
            };
            self.chain_log.push(text_event);

            info!(job_name = %job_name, skill_name = %skill_name, "cron skill chain completed successfully");

            Ok(ChainOutcome {
                events: self.chain_log.clone(),
                success: true,
                summary_text: summary,
            })
        } else {
            let error_msg = format!(
                "[CRON CHAIN FAILED] job={job_name} skill={skill_name} error=SKILL_RESULT_ERROR: {output_or_error}"
            );
            let _ = proto_bridge::build_text_message(&error_msg, &self.agent_id);

            let text_event = ChainEvent::TextMessagePublished {
                content: error_msg.clone(),
                timestamp_ms: proto_bridge::now_ms(),
            };
            self.chain_log.push(text_event);

            // Send Surface notification if sender exists (AC#3)
            if let Some(ref sender) = self.notification_sender {
                let notification = SurfaceNotification {
                    notification_type: NotificationType::SkillFailure,
                    message: error_msg.clone(),
                    metadata: HashMap::from([
                        ("job_name".into(), job_name.clone()),
                        ("skill_name".into(), skill_name.clone()),
                        ("error_code".into(), "SKILL_RESULT_ERROR".into()),
                    ]),
                };
                if let Err(e) = sender.send(notification).await {
                    error!(
                        job_name = %job_name,
                        "failed to send surface notification: {}",
                        e
                    );
                }
            }

            error!(job_name = %job_name, skill_name = %skill_name, "cron skill chain failed");

            Ok(ChainOutcome {
                events: self.chain_log.clone(),
                success: false,
                summary_text: error_msg,
            })
        }
    }

    /// Format the chain log for observability (AC#2 proxy).
    /// Returns ordered event type names with timestamps.
    pub fn format_chain_log(&self) -> Vec<String> {
        self.chain_log
            .iter()
            .map(|e| format!("{} (t={})", e.type_name(), e.timestamp_ms()))
            .collect()
    }

    /// Simulate skill execution. In MVP, skills named "fail-*" or "error-*"
    /// simulate failure; all others succeed.
    fn simulate_skill_execution(&self, skill_name: &str) -> (bool, String) {
        if skill_name.starts_with("fail-") || skill_name.starts_with("error-") {
            (false, format!("simulated failure for skill '{skill_name}'"))
        } else {
            (true, format!("simulated output from skill '{skill_name}'"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::skill_handler::CronSkillHandler;
    use std::sync::Arc;

    /// Helper: set up a chain with a CronSkillHandler for the given skill.
    fn setup_chain(job_name: &str, skill_name: &str) -> (CronSkillChain, mpsc::Sender<ChainEvent>) {
        let (event_tx, event_rx) = mpsc::channel(10);
        let mut dispatcher = CronEventDispatcher::new();
        let handler = Arc::new(CronSkillHandler {
            skill_name: skill_name.into(),
            agent_id: "test-agent".into(),
            event_sender: event_tx.clone(),
        });
        dispatcher.register_handler(job_name, handler);

        let chain = CronSkillChain::new(dispatcher, event_rx, "test-agent");
        (chain, event_tx)
    }

    #[tokio::test]
    async fn successful_chain_produces_events_in_correct_order() {
        let (mut chain, _tx) = setup_chain("echo-cron", "echo");

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

        let outcome = chain.process_cron_event(event).await.unwrap();
        assert!(outcome.success);
        assert_eq!(outcome.events.len(), 5);

        // Verify event order
        assert_eq!(outcome.events[0].type_name(), "CronEvent");
        assert_eq!(outcome.events[1].type_name(), "SkillInvocation");
        assert_eq!(outcome.events[2].type_name(), "SkillProgress");
        assert_eq!(outcome.events[3].type_name(), "SkillResult");
        assert_eq!(outcome.events[4].type_name(), "TextMessage");
    }

    #[tokio::test]
    async fn failed_chain_produces_events_with_error_text_message() {
        let (mut chain, _tx) = setup_chain("fail-cron", "fail-echo");

        let event = CronEventMessage {
            job_name: "fail-cron".into(),
            action: "event".into(),
            schedule: "* * * * *".into(),
            triggered_at: prost_types::Timestamp {
                seconds: 0,
                nanos: 0,
            },
            payload: HashMap::new(),
            target_stream: "test-stream".into(),
        };

        let outcome = chain.process_cron_event(event).await.unwrap();
        assert!(!outcome.success);
        assert_eq!(outcome.events.len(), 5);

        // Verify event order includes error path
        assert_eq!(outcome.events[0].type_name(), "CronEvent");
        assert_eq!(outcome.events[1].type_name(), "SkillInvocation");
        assert_eq!(outcome.events[2].type_name(), "SkillProgress");
        assert_eq!(outcome.events[3].type_name(), "SkillResult");
        assert_eq!(outcome.events[4].type_name(), "TextMessage");

        // Verify SkillResult is a failure
        match &outcome.events[3] {
            ChainEvent::SkillResultReceived { success, .. } => assert!(!success),
            _ => panic!("expected SkillResultReceived"),
        }

        // Verify TextMessage contains error info
        match &outcome.events[4] {
            ChainEvent::TextMessagePublished { content, .. } => {
                assert!(content.contains("CRON CHAIN FAILED"));
                assert!(content.contains("fail-echo"));
            }
            _ => panic!("expected TextMessagePublished"),
        }
    }

    #[tokio::test]
    async fn all_events_have_monotonically_increasing_timestamps() {
        let (mut chain, _tx) = setup_chain("echo-cron", "echo");

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

        let outcome = chain.process_cron_event(event).await.unwrap();

        for window in outcome.events.windows(2) {
            assert!(
                window[1].timestamp_ms() >= window[0].timestamp_ms(),
                "timestamps not monotonically increasing: {} < {}",
                window[1].timestamp_ms(),
                window[0].timestamp_ms()
            );
        }
    }

    #[tokio::test]
    async fn skill_failure_sends_surface_notification() {
        let (mut chain, _tx) = setup_chain("fail-cron", "fail-echo");
        let (notif_tx, mut notif_rx) = mpsc::channel(10);
        chain.set_notification_sender(notif_tx);

        let event = CronEventMessage {
            job_name: "fail-cron".into(),
            action: "event".into(),
            schedule: "* * * * *".into(),
            triggered_at: prost_types::Timestamp {
                seconds: 0,
                nanos: 0,
            },
            payload: HashMap::new(),
            target_stream: "test-stream".into(),
        };

        let outcome = chain.process_cron_event(event).await.unwrap();
        assert!(!outcome.success);

        // Verify notification was sent
        let notification = notif_rx.recv().await.unwrap();
        assert_eq!(
            notification.notification_type,
            NotificationType::SkillFailure
        );
        assert_eq!(notification.metadata.get("job_name").unwrap(), "fail-cron");
        assert_eq!(
            notification.metadata.get("skill_name").unwrap(),
            "fail-echo"
        );
        assert_eq!(
            notification.metadata.get("error_code").unwrap(),
            "SKILL_RESULT_ERROR"
        );
    }

    #[tokio::test]
    async fn chain_without_notification_sender_succeeds() {
        let (mut chain, _tx) = setup_chain("fail-cron", "fail-echo");
        // Do NOT set notification_sender — test graceful degradation

        let event = CronEventMessage {
            job_name: "fail-cron".into(),
            action: "event".into(),
            schedule: "* * * * *".into(),
            triggered_at: prost_types::Timestamp {
                seconds: 0,
                nanos: 0,
            },
            payload: HashMap::new(),
            target_stream: "test-stream".into(),
        };

        let outcome = chain.process_cron_event(event).await.unwrap();
        assert!(!outcome.success);
        // Chain still completes despite no notification sender
        assert_eq!(outcome.events.len(), 5);
    }
}
