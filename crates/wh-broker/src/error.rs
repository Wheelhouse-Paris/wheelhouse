//! Broker error types.
//!
//! Library modules use typed errors with `thiserror` (SCV-04).
//! `anyhow` is only permitted in `main.rs` and tests.

#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error("Failed to bind socket on {endpoint}: {source}")]
    BindError {
        endpoint: String,
        source: zeromq::ZmqError,
    },

    #[error("Control socket error: {0}")]
    ControlError(String),

    #[error("Routing loop error: {0}")]
    RoutingError(String),

    #[error("Wheelhouse is already running or port {port} is in use")]
    PortInUse { port: u16 },

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("WAL error: {0}")]
    WalError(#[from] crate::wal::WalError),

    #[error("Stream error: {0}")]
    StreamError(#[from] crate::metrics::StreamError),
}

/// Typed error for control socket handlers.
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("Unknown command: {0}")]
    UnknownCommand(String),

    #[error("Rate limited")]
    RateLimited,

    #[error("Internal error: {0}")]
    Internal(String),
}
