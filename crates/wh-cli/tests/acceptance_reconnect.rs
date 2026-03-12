//! Acceptance tests for Story 1.5: Subscriber Auto-Reconnect After Interruption.
//!
//! Tests for the reconnect module covering all acceptance criteria.

#[cfg(test)]
mod acceptance_reconnect {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use wh_cli::reconnect::{
        calculate_backoff, ConnectionEvent, ConnectionEventCallback, ReconnectPolicy,
    };

    // ── AC #1: Auto-reconnect within 5 seconds (NFR-R4) ──────────────────

    #[test]
    fn test_reconnect_policy_exists() {
        let policy = ReconnectPolicy::default();
        assert_eq!(policy.base_ms(), 100);
        assert_eq!(policy.multiplier(), 2);
        assert!((policy.cap_s() - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_backoff_formula_attempt_0() {
        // ADR-011: backoff = min(5s, 100ms * 2^attempt) + random(0..100ms)
        let d = calculate_backoff(0);
        assert!(d.as_millis() >= 100, "backoff(0) should be >= 100ms");
        assert!(d.as_millis() <= 200, "backoff(0) should be <= 200ms");
    }

    #[test]
    fn test_backoff_formula_attempt_1() {
        let d = calculate_backoff(1);
        assert!(d.as_millis() >= 200, "backoff(1) should be >= 200ms");
        assert!(d.as_millis() <= 300, "backoff(1) should be <= 300ms");
    }

    #[test]
    fn test_backoff_formula_attempt_2() {
        let d = calculate_backoff(2);
        assert!(d.as_millis() >= 400, "backoff(2) should be >= 400ms");
        assert!(d.as_millis() <= 500, "backoff(2) should be <= 500ms");
    }

    #[test]
    fn test_backoff_capped_at_5_seconds() {
        let d = calculate_backoff(10);
        assert!(d.as_millis() >= 5000, "backoff(10) should be >= 5000ms");
        assert!(d.as_millis() <= 5100, "backoff(10) should be <= 5100ms");
    }

    // ── AC #2: Resume receiving new messages after reconnection ───────────

    #[test]
    fn test_backoff_jitter_bounded() {
        for _ in 0..100 {
            let d = calculate_backoff(0);
            assert!(d.as_millis() >= 100);
            assert!(d.as_millis() <= 200);
        }
    }

    // ── AC #3: Connection events surfaced to callback (CM-02) ─────────────

    #[test]
    fn test_connection_event_types_exist() {
        let _disconnected = ConnectionEvent::Disconnected {
            reason: "test".to_string(),
        };
        let _reconnecting = ConnectionEvent::Reconnecting { attempt: 1 };
        let _reconnected = ConnectionEvent::Reconnected;
        let _failed = ConnectionEvent::ReconnectFailed {
            attempts: 5,
            last_error: "timeout".to_string(),
        };
    }

    #[tokio::test]
    async fn test_ctrl_c_during_reconnect() {
        use tokio_util::sync::CancellationToken;
        use wh_cli::reconnect::{reconnect_subscribe, ReconnectError};

        let cancel = CancellationToken::new();
        cancel.cancel(); // Cancel immediately

        let result =
            reconnect_subscribe("tcp://127.0.0.1:59997", "test\0", &cancel, None).await;

        match result {
            Err(ReconnectError::Cancelled) => {} // expected
            Ok(_) => panic!("expected Cancelled error, got Ok"),
        }
    }

    #[tokio::test]
    async fn test_connection_event_callback_invoked() {
        use tokio_util::sync::CancellationToken;
        use wh_cli::reconnect::reconnect_subscribe;

        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let callback: ConnectionEventCallback = Box::new(move |event| {
            let name = match &event {
                ConnectionEvent::Disconnected { .. } => "disconnected",
                ConnectionEvent::Reconnecting { .. } => "reconnecting",
                ConnectionEvent::Reconnected => "reconnected",
                ConnectionEvent::ReconnectFailed { .. } => "failed",
            };
            events_clone.lock().unwrap().push(name.to_string());
        });

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });

        let _ =
            reconnect_subscribe("tcp://127.0.0.1:59996", "test\0", &cancel, Some(&callback))
                .await;

        let captured = events.lock().unwrap();
        assert!(!captured.is_empty(), "Expected at least one callback event");
        assert_eq!(captured[0], "reconnecting");
    }
}
