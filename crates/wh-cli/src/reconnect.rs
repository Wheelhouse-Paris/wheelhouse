//! Subscriber auto-reconnect with exponential backoff (Story 1.5).
//!
//! Implements ADR-011 reconnect backoff: `min(5s, 100ms * 2^attempt) + random(0..100ms)`.
//! Provides `ConnectionEvent` for Surface notification callbacks (CM-02).

use std::time::Duration;

use rand::Rng;
use tokio_util::sync::CancellationToken;
use zeromq::{Socket, SubSocket};

/// Reconnect backoff constants (ADR-011, FM-13).
/// Fixed constants — not configurable.
const BASE_MS: u64 = 100;
const MULTIPLIER: u64 = 2;
const CAP_MS: u64 = 5_000;
const JITTER_MAX_MS: u64 = 100;

/// Reconnect policy with fixed ADR-011 constants.
#[derive(Default)]
pub struct ReconnectPolicy {
    _private: (),
}

impl ReconnectPolicy {
    /// Base backoff in milliseconds.
    pub fn base_ms(&self) -> u64 {
        BASE_MS
    }

    /// Backoff multiplier.
    pub fn multiplier(&self) -> u64 {
        MULTIPLIER
    }

    /// Maximum backoff cap in seconds.
    pub fn cap_s(&self) -> f64 {
        CAP_MS as f64 / 1000.0
    }
}

/// Calculate reconnect backoff delay for a given attempt number.
///
/// Formula: `min(5s, 100ms * 2^attempt) + random(0..100ms)` (ADR-011, FM-13).
pub fn calculate_backoff(attempt: u32) -> Duration {
    let base = BASE_MS.saturating_mul(MULTIPLIER.saturating_pow(attempt));
    let capped = base.min(CAP_MS);
    let jitter = rand::thread_rng().gen_range(0..=JITTER_MAX_MS);
    Duration::from_millis(capped + jitter)
}

/// Connection lifecycle events for Surface notification (CM-02).
///
/// Callbacks receive these events at each state transition during
/// the reconnect loop. User-facing text avoids "broker" (RT-B1).
#[derive(Debug, Clone)]
pub enum ConnectionEvent {
    /// Connection to Wheelhouse was lost.
    Disconnected {
        /// Human-readable reason (never says "broker").
        reason: String,
    },
    /// A reconnect attempt is starting.
    Reconnecting {
        /// 1-based attempt number.
        attempt: u32,
    },
    /// Successfully reconnected to Wheelhouse.
    Reconnected,
    /// Reconnect failed (informational — retries continue).
    ReconnectFailed {
        /// Total attempts so far.
        attempts: u32,
        /// Last error description.
        last_error: String,
    },
}

/// Type alias for connection event callbacks.
pub type ConnectionEventCallback = Box<dyn Fn(ConnectionEvent) + Send>;

/// Attempt to reconnect a ZMQ SUB socket with exponential backoff.
///
/// Loops until either:
/// - Connection succeeds (returns new SubSocket)
/// - Cancellation token fires (returns Err)
///
/// Invokes the optional callback at each state transition (CM-02).
pub async fn reconnect_subscribe(
    endpoint: &str,
    topic: &str,
    cancel: &CancellationToken,
    on_event: Option<&ConnectionEventCallback>,
) -> Result<SubSocket, ReconnectError> {
    let mut attempt: u32 = 0;

    loop {
        attempt = attempt.saturating_add(1);

        if let Some(cb) = on_event {
            cb(ConnectionEvent::Reconnecting { attempt });
        }

        let delay = calculate_backoff(attempt.saturating_sub(1));

        tracing::info!(
            attempt = attempt,
            delay_ms = delay.as_millis() as u64,
            "Reconnecting to Wheelhouse"
        );

        // Wait with cancellation support (Ctrl+C terminates during backoff)
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                return Err(ReconnectError::Cancelled);
            }
            _ = tokio::time::sleep(delay) => {}
        }

        // Attempt new connection
        let mut sub_socket = SubSocket::new();
        match sub_socket.connect(endpoint).await {
            Ok(()) => {
                // Re-subscribe to topic
                match sub_socket.subscribe(topic).await {
                    Ok(()) => {
                        if let Some(cb) = on_event {
                            cb(ConnectionEvent::Reconnected);
                        }
                        tracing::info!(
                            endpoint = endpoint,
                            attempt = attempt,
                            "Reconnected to Wheelhouse"
                        );
                        return Ok(sub_socket);
                    }
                    Err(e) => {
                        let err_msg = format!("Subscribe failed: {e}");
                        if let Some(cb) = on_event {
                            cb(ConnectionEvent::ReconnectFailed {
                                attempts: attempt,
                                last_error: err_msg.clone(),
                            });
                        }
                        tracing::warn!(
                            error = %err_msg,
                            attempt = attempt,
                            "Reconnect subscribe failed"
                        );
                        continue;
                    }
                }
            }
            Err(e) => {
                let err_msg = format!("Connection failed: {e}");
                if let Some(cb) = on_event {
                    cb(ConnectionEvent::ReconnectFailed {
                        attempts: attempt,
                        last_error: err_msg.clone(),
                    });
                }
                tracing::warn!(
                    error = %err_msg,
                    attempt = attempt,
                    "Reconnect failed"
                );
                continue;
            }
        }
    }
}

/// Errors from the reconnect loop.
#[derive(Debug, thiserror::Error)]
pub enum ReconnectError {
    /// Reconnect was cancelled (e.g., Ctrl+C).
    #[error("reconnect cancelled")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backoff_attempt_0() {
        let d = calculate_backoff(0);
        assert!(d.as_millis() >= 100, "backoff(0) should be >= 100ms, got {}ms", d.as_millis());
        assert!(d.as_millis() <= 200, "backoff(0) should be <= 200ms, got {}ms", d.as_millis());
    }

    #[test]
    fn test_backoff_attempt_1() {
        let d = calculate_backoff(1);
        assert!(d.as_millis() >= 200, "backoff(1) should be >= 200ms, got {}ms", d.as_millis());
        assert!(d.as_millis() <= 300, "backoff(1) should be <= 300ms, got {}ms", d.as_millis());
    }

    #[test]
    fn test_backoff_attempt_2() {
        let d = calculate_backoff(2);
        assert!(d.as_millis() >= 400, "backoff(2) should be >= 400ms, got {}ms", d.as_millis());
        assert!(d.as_millis() <= 500, "backoff(2) should be <= 500ms, got {}ms", d.as_millis());
    }

    #[test]
    fn test_backoff_capped_at_5s() {
        let d = calculate_backoff(10);
        assert!(d.as_millis() >= 5000, "backoff(10) should be >= 5000ms, got {}ms", d.as_millis());
        assert!(d.as_millis() <= 5100, "backoff(10) should be <= 5100ms, got {}ms", d.as_millis());
    }

    #[test]
    fn test_backoff_jitter_bounded() {
        for _ in 0..100 {
            let d = calculate_backoff(0);
            assert!(d.as_millis() >= 100);
            assert!(d.as_millis() <= 200);
        }
    }

    #[test]
    fn test_reconnect_policy_defaults() {
        let policy = ReconnectPolicy::default();
        assert_eq!(policy.base_ms(), 100);
        assert_eq!(policy.multiplier(), 2);
        assert!((policy.cap_s() - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_connection_event_variants() {
        let _d = ConnectionEvent::Disconnected { reason: "test".into() };
        let _r = ConnectionEvent::Reconnecting { attempt: 1 };
        let _c = ConnectionEvent::Reconnected;
        let _f = ConnectionEvent::ReconnectFailed {
            attempts: 5,
            last_error: "timeout".into(),
        };
    }

    #[test]
    fn test_backoff_saturation_no_panic() {
        // Very high attempt should not panic (overflow protection)
        let d = calculate_backoff(100);
        assert!(d.as_millis() >= 5000);
        assert!(d.as_millis() <= 5100);
    }

    #[tokio::test]
    async fn test_reconnect_cancelled() {
        let cancel = CancellationToken::new();
        cancel.cancel();

        let result = reconnect_subscribe(
            "tcp://127.0.0.1:59999",
            "test\0",
            &cancel,
            None,
        )
        .await;

        match result {
            Err(ReconnectError::Cancelled) => {} // expected
            Ok(_) => panic!("expected Cancelled error, got Ok"),
        }
    }

    #[tokio::test]
    async fn test_reconnect_callback_invoked() {
        use std::sync::{Arc, Mutex};

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

        // Cancel very quickly — we just need to verify the first callback fires
        // before the backoff sleep completes (attempt 0 = 100ms base)
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });

        let _ = reconnect_subscribe(
            "tcp://127.0.0.1:59998",
            "test\0",
            &cancel,
            Some(&callback),
        )
        .await;

        let captured = events.lock().unwrap();
        // Should have at least one "reconnecting" event (fired before backoff sleep)
        assert!(!captured.is_empty(), "Expected at least one callback event");
        assert_eq!(captured[0], "reconnecting");
    }
}
