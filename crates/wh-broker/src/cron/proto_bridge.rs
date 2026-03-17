//! Proto type builders for the cron -> skill invocation chain.
//!
//! Converts between internal types and protobuf types, generates UUIDs,
//! and formats chain summaries.

use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use super::chain::ChainOutcome;
use super::CronEventMessage;

/// Current UTC epoch milliseconds.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_millis() as i64
}

/// Build a SkillInvocation proto from a CronEventMessage.
/// Generates a unique invocation_id via UUID v4.
pub fn build_skill_invocation_from_cron(
    event: &CronEventMessage,
    skill_name: &str,
    agent_id: &str,
) -> wh_proto::SkillInvocation {
    wh_proto::SkillInvocation {
        skill_name: skill_name.to_string(),
        agent_id: agent_id.to_string(),
        invocation_id: Uuid::new_v4().to_string(),
        parameters: event.payload.clone(),
        timestamp_ms: now_ms(),
    }
}

/// Build a TextMessage proto.
pub fn build_text_message(content: &str, publisher_id: &str) -> wh_proto::TextMessage {
    wh_proto::TextMessage {
        content: content.to_string(),
        publisher_id: publisher_id.to_string(),
        timestamp_ms: now_ms(),
        user_id: String::new(),
        reply_to_user_id: String::new(),
        source_stream: String::new(),
        source_topic: String::new(),
    }
}

/// Build a SkillProgress proto (CM-06).
pub fn build_skill_progress(
    invocation_id: &str,
    skill_name: &str,
    percent: u32,
    message: &str,
) -> wh_proto::SkillProgress {
    wh_proto::SkillProgress {
        invocation_id: invocation_id.to_string(),
        skill_name: skill_name.to_string(),
        progress_percent: percent as f32,
        status_message: message.to_string(),
        timestamp_ms: now_ms(),
    }
}

/// Format a ChainOutcome into a human-readable summary for TextMessage content.
pub fn build_chain_summary(outcome: &ChainOutcome) -> String {
    let status = if outcome.success { "SUCCESS" } else { "FAILED" };
    let event_list: Vec<String> = outcome
        .events
        .iter()
        .map(|e| format!("  {} (t={})", e.type_name(), e.timestamp_ms()))
        .collect();

    format!(
        "[CRON CHAIN {}] {}\nEvents:\n{}",
        status,
        outcome.summary_text,
        event_list.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::chain::ChainEvent;

    use std::collections::HashMap;

    #[test]
    fn build_skill_invocation_has_correct_fields() {
        let event = CronEventMessage {
            job_name: "echo-cron".into(),
            action: "event".into(),
            schedule: "* * * * *".into(),
            triggered_at: prost_types::Timestamp {
                seconds: 0,
                nanos: 0,
            },
            payload: [("input".into(), "hello".into())].into_iter().collect(),
            target_stream: "test-stream".into(),
        };

        let inv = build_skill_invocation_from_cron(&event, "echo", "agent-1");
        assert_eq!(inv.skill_name, "echo");
        assert_eq!(inv.agent_id, "agent-1");
        assert!(!inv.invocation_id.is_empty());
        assert_eq!(inv.parameters.get("input").unwrap(), "hello");
        assert!(inv.timestamp_ms > 0);
    }

    #[test]
    fn build_skill_invocation_generates_unique_ids() {
        let event = CronEventMessage {
            job_name: "test".into(),
            action: "event".into(),
            schedule: "* * * * *".into(),
            triggered_at: prost_types::Timestamp {
                seconds: 0,
                nanos: 0,
            },
            payload: HashMap::new(),
            target_stream: "test-stream".into(),
        };

        let inv1 = build_skill_invocation_from_cron(&event, "echo", "agent-1");
        let inv2 = build_skill_invocation_from_cron(&event, "echo", "agent-1");
        assert_ne!(inv1.invocation_id, inv2.invocation_id);
    }

    #[test]
    fn build_text_message_has_correct_fields() {
        let msg = build_text_message("hello world", "agent-1");
        assert_eq!(msg.content, "hello world");
        assert_eq!(msg.publisher_id, "agent-1");
        assert!(msg.timestamp_ms > 0);
    }

    #[test]
    fn build_skill_progress_has_correct_fields() {
        let progress = build_skill_progress("inv-1", "echo", 0, "started");
        assert_eq!(progress.invocation_id, "inv-1");
        assert_eq!(progress.skill_name, "echo");
        assert!((progress.progress_percent - 0.0_f32).abs() < f32::EPSILON);
        assert_eq!(progress.status_message, "started");
        assert!(progress.timestamp_ms > 0);
    }

    #[test]
    fn build_chain_summary_includes_all_event_types() {
        let outcome = ChainOutcome {
            events: vec![
                ChainEvent::CronEventReceived {
                    job_name: "test".into(),
                    timestamp_ms: 100,
                },
                ChainEvent::SkillInvocationPublished {
                    invocation_id: "inv-1".into(),
                    skill_name: "echo".into(),
                    timestamp_ms: 200,
                },
                ChainEvent::SkillProgressPublished {
                    invocation_id: "inv-1".into(),
                    percent: 0,
                    message: "started".into(),
                    timestamp_ms: 300,
                },
                ChainEvent::SkillResultReceived {
                    invocation_id: "inv-1".into(),
                    success: true,
                    output_or_error: "ok".into(),
                    timestamp_ms: 400,
                },
                ChainEvent::TextMessagePublished {
                    content: "summary".into(),
                    timestamp_ms: 500,
                },
            ],
            success: true,
            summary_text: "echo-cron completed".into(),
        };

        let summary = build_chain_summary(&outcome);
        assert!(summary.contains("SUCCESS"));
        assert!(summary.contains("CronEvent"));
        assert!(summary.contains("SkillInvocation"));
        assert!(summary.contains("SkillProgress"));
        assert!(summary.contains("SkillResult"));
        assert!(summary.contains("TextMessage"));
    }
}
