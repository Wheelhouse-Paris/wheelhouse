//! Control socket client (JSON-over-ZMQ REQ) connecting to `.wh/control.sock`.
//!
//! The CLI connects as a REQ client to the broker's control socket (ADR-010).
//! Connection failures are mapped to user-friendly messages using approved
//! vocabulary — never "broker", "connection refused", or port numbers (RT-B1).

use std::path::PathBuf;

use crate::output::error::WhError;

/// Default control socket path relative to working directory.
const CONTROL_SOCK_RELATIVE: &str = ".wh/control.sock";

// [PHASE-2-ONLY: zmq-recv-timeout] Timeout for receiving a response from the control socket (CF-02).
// const RECV_TIMEOUT_SECS: u64 = 5;

/// Control socket client for communicating with the running Wheelhouse instance.
pub struct ControlClient {
    _socket_path: PathBuf,
}

impl ControlClient {
    /// Attempt to connect to the control socket.
    ///
    /// Returns `Err(WhError::ConnectionError)` if:
    /// - `.wh/` directory does not exist (never deployed)
    /// - `.wh/control.sock` does not exist (Wheelhouse not running)
    /// - Connection to socket fails
    pub fn connect() -> Result<Self, WhError> {
        let socket_path = PathBuf::from(CONTROL_SOCK_RELATIVE);

        // Check if .wh/ directory exists
        let wh_dir = PathBuf::from(".wh");
        if !wh_dir.exists() {
            return Err(WhError::ConnectionError);
        }

        // Check if control socket file exists
        if !socket_path.exists() {
            return Err(WhError::ConnectionError);
        }

        // [PHASE-2-ONLY: zmq-control-client] Full ZMQ REQ/REP implementation.
        // For now, we verify the socket path exists but actual ZMQ connection
        // requires the broker to be running. Since the broker is not yet
        // implemented (Epic 1), this will always fail at the exists() check.
        Ok(Self {
            _socket_path: socket_path,
        })
    }

    /// Send a command to the control socket and receive a JSON response.
    ///
    /// The command is a JSON object sent as a ZMQ message.
    /// The response is a JSON object with `"v": 1` schema version.
    pub fn send_command(&self, _command: &str) -> Result<serde_json::Value, WhError> {
        // [PHASE-2-ONLY: zmq-control-client] Actual ZMQ send/recv implementation.
        // This requires the broker's control socket server (Epic 1, Story 1.2).
        Err(WhError::ConnectionError)
    }
}
