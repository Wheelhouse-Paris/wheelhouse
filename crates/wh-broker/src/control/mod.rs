//! Control socket listener (ADR-010, RT-02).
//!
//! Runs as a separate tokio task. Handles JSON-over-ZMQ REQ/REP commands.
//! Implements rate limiting: 10 req/s via sliding window (RT-02).

pub mod handlers;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;
use tokio_util::sync::CancellationToken;
use zeromq::{RepSocket, Socket, SocketRecv, SocketSend, ZmqMessage};

use crate::config::BrokerConfig;
use crate::error::BrokerError;
use crate::metrics::BrokerState;

/// Maximum requests per second per client (RT-02).
const RATE_LIMIT_MAX_REQUESTS: usize = 10;
/// Sliding window duration for rate limiting.
const RATE_LIMIT_WINDOW_SECS: u64 = 1;

/// Simple sliding window rate limiter.
pub struct RateLimiter {
    /// Maps client identifier to list of request timestamps.
    windows: HashMap<String, Vec<Instant>>,
    max_requests: usize,
    window_secs: u64,
}

impl RateLimiter {
    pub fn new(max_requests: usize, window_secs: u64) -> Self {
        Self {
            windows: HashMap::new(),
            max_requests,
            window_secs,
        }
    }

    /// Check if a request from the given client is allowed.
    /// Returns `true` if allowed, `false` if rate limited.
    pub fn check(&mut self, client_id: &str) -> bool {
        let now = Instant::now();
        let window = self.windows.entry(client_id.to_string()).or_default();

        // Remove timestamps outside the window
        let cutoff = now - std::time::Duration::from_secs(self.window_secs);
        window.retain(|t| *t > cutoff);

        if window.len() >= self.max_requests {
            return false;
        }

        window.push(now);
        true
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(RATE_LIMIT_MAX_REQUESTS, RATE_LIMIT_WINDOW_SECS)
    }
}

/// Run the control socket listener task.
///
/// Binds a REP socket and handles incoming commands via the dispatch table.
#[tracing::instrument(skip_all, fields(endpoint = %config.control_endpoint()))]
pub async fn run_control_loop(
    config: &BrokerConfig,
    state: Arc<BrokerState>,
    cancel: CancellationToken,
) -> Result<(), BrokerError> {
    let mut socket = RepSocket::new();
    socket
        .bind(config.control_endpoint().as_str())
        .await
        .map_err(|e| BrokerError::BindError {
            endpoint: config.control_endpoint(),
            source: e,
        })?;

    tracing::info!(
        endpoint = %config.control_endpoint(),
        "control socket bound on 127.0.0.1"
    );

    let mut rate_limiter = RateLimiter::default();

    loop {
        tokio::select! {
            biased;

            _ = cancel.cancelled() => {
                tracing::info!("control socket shutting down");
                break;
            }

            result = socket.recv() => {
                match result {
                    Ok(msg) => {
                        let response = handle_message(&msg, &state, &mut rate_limiter).await;
                        let response_bytes = serde_json::to_vec(&response)
                            .unwrap_or_else(|_| b"{}".to_vec());
                        let reply = ZmqMessage::from(response_bytes);
                        if let Err(e) = socket.send(reply).await {
                            tracing::warn!(error = %e, "failed to send control response");
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "control socket recv error");
                        tokio::task::yield_now().await;
                    }
                }
            }
        }
    }

    Ok(())
}

/// Parse and handle a single control message.
async fn handle_message(
    msg: &ZmqMessage,
    state: &Arc<BrokerState>,
    rate_limiter: &mut RateLimiter,
) -> Value {
    // Use "default" as client identifier (no per-client auth in MVP)
    let client_id = "default";

    // Check rate limit (RT-02)
    if !rate_limiter.check(client_id) {
        return handlers::error_response("RATE_LIMITED", "Too many requests");
    }

    // Parse the command from the message
    let request_bytes: Vec<u8> = msg.clone().try_into().unwrap_or_default();
    let request_str = String::from_utf8_lossy(&request_bytes);

    let command = match serde_json::from_str::<Value>(&request_str) {
        Ok(v) => v
            .get("command")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string(),
        Err(_) => {
            return handlers::error_response("INVALID_REQUEST", "Invalid JSON request");
        }
    };

    match handlers::dispatch(&command, state).await {
        Ok(response) => response,
        Err(crate::error::ControlError::UnknownCommand(cmd)) => {
            handlers::error_response("UNKNOWN_COMMAND", &format!("Unknown command: {cmd}"))
        }
        Err(crate::error::ControlError::RateLimited) => {
            handlers::error_response("RATE_LIMITED", "Too many requests")
        }
        Err(crate::error::ControlError::Internal(msg)) => {
            handlers::error_response("INTERNAL_ERROR", &msg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_allows_within_limit() {
        let mut limiter = RateLimiter::new(10, 1);
        for _ in 0..10 {
            assert!(limiter.check("client1"));
        }
    }

    #[test]
    fn test_rate_limiter_blocks_over_limit() {
        let mut limiter = RateLimiter::new(10, 1);
        for _ in 0..10 {
            assert!(limiter.check("client1"));
        }
        // 11th request should be blocked
        assert!(!limiter.check("client1"));
    }

    #[test]
    fn test_rate_limiter_separate_clients() {
        let mut limiter = RateLimiter::new(10, 1);
        for _ in 0..10 {
            assert!(limiter.check("client1"));
        }
        // Different client should still be allowed
        assert!(limiter.check("client2"));
    }
}
