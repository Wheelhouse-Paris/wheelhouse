//! Control socket client for CLI-to-broker communication (ADR-010).
//!
//! Connects via ZMQ REQ socket to the broker's REP control socket.
//! 5-second receive timeout (CF-02).

use serde_json::{json, Value};
use zeromq::{ReqSocket, Socket, SocketRecv, SocketSend, ZmqMessage};

use crate::output::error::WhError;

/// Receive timeout in milliseconds (CF-02).
const RECV_TIMEOUT_MS: u64 = 5000;

/// Control socket client.
pub struct ControlClient {
    endpoint: String,
}

impl ControlClient {
    /// Create a new client connecting to the default or env-specified endpoint.
    pub fn new() -> Self {
        let endpoint = std::env::var("WH_CONTROL_ENDPOINT").unwrap_or_else(|_| {
            let port = std::env::var("WH_CONTROL_PORT")
                .ok()
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(5557);
            format!("tcp://127.0.0.1:{port}")
        });

        Self { endpoint }
    }

    /// Send a command and receive the response.
    ///
    /// Returns the parsed JSON response or a `WhError`.
    pub async fn send_command(&self, command: &str) -> Result<Value, WhError> {
        let request = json!({"command": command});
        self.send_command_with_payload(request).await
    }

    /// Send a command with a full JSON payload and receive the response.
    ///
    /// The payload should include the "command" field plus any additional fields
    /// needed by the handler (e.g., "name", "retention" for stream commands).
    pub async fn send_command_with_payload(&self, payload: Value) -> Result<Value, WhError> {
        let mut socket = ReqSocket::new();
        socket
            .connect(&self.endpoint)
            .await
            .map_err(|_| WhError::ConnectionError)?;

        let request_bytes = serde_json::to_vec(&payload)
            .map_err(|e| WhError::Other(format!("Failed to serialize request: {e}")))?;

        let msg = ZmqMessage::from(request_bytes);
        socket
            .send(msg)
            .await
            .map_err(|_| WhError::ConnectionError)?;

        // Receive with timeout (CF-02)
        let response = tokio::time::timeout(
            std::time::Duration::from_millis(RECV_TIMEOUT_MS),
            socket.recv(),
        )
        .await
        .map_err(|_| WhError::Timeout)?
        .map_err(|_| WhError::ConnectionError)?;

        let response_bytes: Vec<u8> = response.try_into().unwrap_or_default();
        let response_str = String::from_utf8_lossy(&response_bytes);

        serde_json::from_str(&response_str)
            .map_err(|e| WhError::InvalidResponse(format!("Invalid JSON: {e}")))
    }
}

impl Default for ControlClient {
    fn default() -> Self {
        Self::new()
    }
}
