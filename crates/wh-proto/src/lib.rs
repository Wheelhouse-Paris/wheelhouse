pub mod wheelhouse {
    pub mod v1 {
        include!(concat!(env!("OUT_DIR"), "/wheelhouse.v1.rs"));
    }
}

#[cfg(test)]
mod tests {
    use super::wheelhouse::v1::CronEvent;
    use prost::Message;
    use std::collections::HashMap;

    #[test]
    fn cron_event_roundtrip_serialization() {
        let mut payload = HashMap::new();
        payload.insert("type".to_string(), "briefing".to_string());

        let event = CronEvent {
            job_name: "daily-compaction".to_string(),
            action: "compact".to_string(),
            schedule: "0 3 * * *".to_string(),
            triggered_at: Some(prost_types::Timestamp {
                seconds: 1710288000,
                nanos: 0,
            }),
            payload,
        };

        // Serialize
        let mut buf = Vec::new();
        event.encode(&mut buf).expect("encode should succeed");
        assert!(!buf.is_empty(), "encoded bytes should not be empty");

        // Deserialize
        let decoded = CronEvent::decode(buf.as_slice()).expect("decode should succeed");
        assert_eq!(decoded.job_name, "daily-compaction");
        assert_eq!(decoded.action, "compact");
        assert_eq!(decoded.schedule, "0 3 * * *");
        assert!(decoded.triggered_at.is_some());
        assert_eq!(decoded.triggered_at.unwrap().seconds, 1710288000);
        assert_eq!(decoded.payload.get("type").unwrap(), "briefing");
    }

    #[test]
    fn cron_event_default_fields() {
        let event = CronEvent::default();
        assert_eq!(event.job_name, "");
        assert_eq!(event.action, "");
        assert_eq!(event.schedule, "");
        assert!(event.triggered_at.is_none());
        assert!(event.payload.is_empty());
    }
}
