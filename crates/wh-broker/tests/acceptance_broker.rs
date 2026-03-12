//! Acceptance tests for Story 1.2: Broker Process, Localhost Security, and Health Check.
//!
//! These tests verify the broker's core functionality:
//! - Localhost-only binding (AC#1)
//! - Status command returning valid JSON (AC#2)
//! - Response time under 500ms (AC#2, NFR-P2)
//! - Graceful shutdown (AC#1)
//! - Rate limiting (RT-02)

use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;
use tokio_util::sync::CancellationToken;
use zeromq::{ReqSocket, Socket, SocketRecv, SocketSend, ZmqMessage};

use wh_broker::config::BrokerConfig;
use wh_broker::metrics::BrokerState;

/// Helper: start broker control loop on ephemeral port and return the config.
fn test_config() -> BrokerConfig {
    // Use ephemeral ports to avoid conflicts in CI
    BrokerConfig::with_ports(
        portpicker::pick_unused_port().expect("no free port"),
        portpicker::pick_unused_port().expect("no free port"),
        portpicker::pick_unused_port().expect("no free port"),
    )
}

/// Helper: send a JSON command to the control socket and parse the response.
async fn send_control_command(endpoint: &str, command: &str) -> Value {
    let mut socket = ReqSocket::new();
    socket.connect(endpoint).await.expect("connect failed");

    let request = serde_json::json!({"command": command});
    let request_bytes = serde_json::to_vec(&request).unwrap();
    let msg = ZmqMessage::from(request_bytes);
    socket.send(msg).await.expect("send failed");

    let response = tokio::time::timeout(std::time::Duration::from_secs(5), socket.recv())
        .await
        .expect("timeout")
        .expect("recv failed");

    let response_bytes: Vec<u8> = response.try_into().unwrap_or_default();
    let response_str = String::from_utf8_lossy(&response_bytes);
    serde_json::from_str(&response_str).expect("invalid JSON response")
}

#[tokio::test]
async fn test_broker_binds_localhost_only() {
    let config = test_config();

    // Verify all endpoints use 127.0.0.1
    assert!(
        config.pub_endpoint().contains("127.0.0.1"),
        "PUB endpoint must bind to 127.0.0.1"
    );
    assert!(
        config.sub_endpoint().contains("127.0.0.1"),
        "SUB endpoint must bind to 127.0.0.1"
    );
    assert!(
        config.control_endpoint().contains("127.0.0.1"),
        "Control endpoint must bind to 127.0.0.1"
    );

    // Verify bind address is hardcoded
    assert_eq!(config.bind_address(), "127.0.0.1");
}

#[tokio::test]
async fn test_broker_status_returns_valid_json() {
    let config = test_config();
    let state = BrokerState::new();
    let cancel = CancellationToken::new();

    let control_endpoint = config.control_endpoint();

    // Start control loop
    let control_cancel = cancel.clone();
    let control_config = config.clone();
    let control_state = Arc::clone(&state);
    let handle = tokio::spawn(async move {
        wh_broker::control::run_control_loop(&control_config, control_state, control_cancel).await
    });

    // Give the socket time to bind
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Send status command
    let response = send_control_command(&control_endpoint, "status").await;

    // Verify JSON schema (ADR-010)
    assert_eq!(
        response.get("v").and_then(|v| v.as_u64()),
        Some(1),
        "response must include v:1"
    );
    assert_eq!(
        response.get("status").and_then(|s| s.as_str()),
        Some("ok"),
        "status must be ok"
    );

    let data = response
        .get("data")
        .expect("response must include data field");
    assert!(
        data.get("uptime_secs").is_some(),
        "data must include uptime_secs"
    );
    assert!(
        data.get("panic_count").is_some(),
        "data must include panic_count"
    );
    assert!(data.get("streams").is_some(), "data must include streams");
    assert!(
        data.get("streams").unwrap().as_array().is_some(),
        "streams must be an array"
    );

    // Cleanup
    cancel.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn test_broker_status_under_500ms() {
    let config = test_config();
    let state = BrokerState::new();
    let cancel = CancellationToken::new();

    let control_endpoint = config.control_endpoint();

    let control_cancel = cancel.clone();
    let control_config = config.clone();
    let control_state = Arc::clone(&state);
    let handle = tokio::spawn(async move {
        wh_broker::control::run_control_loop(&control_config, control_state, control_cancel).await
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let start = Instant::now();
    let _response = send_control_command(&control_endpoint, "status").await;
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 500,
        "Status response took {}ms, must be under 500ms (NFR-P2)",
        elapsed.as_millis()
    );

    cancel.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn test_broker_graceful_shutdown() {
    let config = test_config();
    let state = BrokerState::new();
    let cancel = CancellationToken::new();

    let control_cancel = cancel.clone();
    let control_config = config.clone();
    let control_state = Arc::clone(&state);
    let handle = tokio::spawn(async move {
        wh_broker::control::run_control_loop(&control_config, control_state, control_cancel).await
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Trigger shutdown
    cancel.cancel();

    // Should complete within a reasonable time
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    assert!(
        result.is_ok(),
        "Control loop should shut down within 5 seconds"
    );
}

#[tokio::test]
async fn test_control_socket_rate_limiter() {
    let config = test_config();
    let state = BrokerState::new();
    let cancel = CancellationToken::new();

    let control_endpoint = config.control_endpoint();

    let control_cancel = cancel.clone();
    let control_config = config.clone();
    let control_state = Arc::clone(&state);
    let handle = tokio::spawn(async move {
        wh_broker::control::run_control_loop(&control_config, control_state, control_cancel).await
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Send 11 requests rapidly -- the 11th should be rate limited
    // Note: with REQ/REP pattern, we need to do sequential request-reply
    let mut got_rate_limited = false;
    for i in 0..15 {
        let mut socket = ReqSocket::new();
        socket
            .connect(&control_endpoint)
            .await
            .expect("connect failed");

        let request = serde_json::json!({"command": "status"});
        let request_bytes = serde_json::to_vec(&request).unwrap();
        let msg = ZmqMessage::from(request_bytes);
        socket.send(msg).await.expect("send failed");

        let response = tokio::time::timeout(std::time::Duration::from_secs(5), socket.recv())
            .await
            .expect("timeout")
            .expect("recv failed");

        let response_bytes: Vec<u8> = response.try_into().unwrap_or_default();
        let response_str = String::from_utf8_lossy(&response_bytes);
        let json: Value = serde_json::from_str(&response_str).expect("invalid JSON");

        if json.get("code").and_then(|c| c.as_str()) == Some("RATE_LIMITED") {
            got_rate_limited = true;
            // Verify rate limited response has correct schema
            assert_eq!(json.get("v").and_then(|v| v.as_u64()), Some(1));
            assert_eq!(json.get("status").and_then(|s| s.as_str()), Some("error"));
            break;
        }

        // Small delay to not overwhelm ZMQ connection setup
        if i < 10 {
            // First 10 should succeed
            assert_eq!(
                json.get("status").and_then(|s| s.as_str()),
                Some("ok"),
                "Request {i} should succeed"
            );
        }
    }

    assert!(
        got_rate_limited,
        "Rate limiter should have kicked in after 10 requests"
    );

    cancel.cancel();
    let _ = handle.await;
}
