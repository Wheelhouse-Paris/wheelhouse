/// Generated protobuf types for Wheelhouse.
pub mod wheelhouse {
    pub mod v1 {
        include!(concat!(env!("OUT_DIR"), "/wheelhouse.v1.rs"));
    }
}

// Re-export commonly used types at crate root for convenience.
pub use wheelhouse::v1::CronEvent;
pub use wheelhouse::v1::SkillInvocation;
pub use wheelhouse::v1::SkillProgress;
pub use wheelhouse::v1::SkillResult;
pub use wheelhouse::v1::TextMessage;

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    #[test]
    fn cron_event_roundtrip() {
        let event = CronEvent {
            job_name: "daily-compaction".into(),
            action: "compact".into(),
            schedule: "0 3 * * *".into(),
            triggered_at: None,
            payload: [("key".into(), "value".into())].into_iter().collect(),
        };
        let bytes = event.encode_to_vec();
        let decoded = CronEvent::decode(bytes.as_slice()).unwrap();
        assert_eq!(decoded.job_name, "daily-compaction");
        assert_eq!(decoded.action, "compact");
        assert_eq!(decoded.payload.get("key").unwrap(), "value");
    }

    #[test]
    fn skill_invocation_roundtrip() {
        let inv = SkillInvocation {
            skill_name: "echo".into(),
            agent_id: "agent-1".into(),
            invocation_id: "inv-001".into(),
            parameters: [("input".into(), "hello".into())].into_iter().collect(),
            timestamp_ms: 1234567890,
        };
        let bytes = inv.encode_to_vec();
        let decoded = SkillInvocation::decode(bytes.as_slice()).unwrap();
        assert_eq!(decoded.skill_name, "echo");
        assert_eq!(decoded.invocation_id, "inv-001");
    }

    #[test]
    fn text_message_roundtrip() {
        let msg = TextMessage {
            content: "hello world".into(),
            author_id: "agent-1".into(),
            timestamp_ms: 9999,
            metadata: Default::default(),
        };
        let bytes = msg.encode_to_vec();
        let decoded = TextMessage::decode(bytes.as_slice()).unwrap();
        assert_eq!(decoded.content, "hello world");
        assert_eq!(decoded.timestamp_ms, 9999);
    }
}
