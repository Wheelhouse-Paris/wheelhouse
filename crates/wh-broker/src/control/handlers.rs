//! Control socket command dispatch table (ADR-010).
//!
//! Each handler returns a JSON response with `"v": 1` schema version.
//! Dispatch table kept minimal (~30 lines max per architecture).

use std::sync::Arc;

use serde_json::{json, Value};

use crate::error::ControlError;
use crate::metrics::BrokerState;

/// Dispatch a control command to the appropriate handler.
///
/// Returns the JSON response value to send back to the client.
pub async fn dispatch(command: &str, state: &Arc<BrokerState>) -> Result<Value, ControlError> {
    match command {
        "status" => handle_status(state).await,
        other => Err(ControlError::UnknownCommand(other.to_string())),
    }
}

/// Handle the `status` command -- returns broker health data (AC#2).
///
/// Response matches canonical `wh status` JSON schema (SCV-02).
#[tracing::instrument(skip_all)]
async fn handle_status(state: &Arc<BrokerState>) -> Result<Value, ControlError> {
    let uptime_secs = state.metrics.uptime_secs();
    let panic_count = state.metrics.get_panic_count();
    let subscriber_count = *state.subscriber_count.read().await;

    Ok(json!({
        "v": 1,
        "status": "ok",
        "data": {
            "uptime_secs": uptime_secs,
            "panic_count": panic_count,
            "subscriber_count": subscriber_count,
            "streams": []
        }
    }))
}

/// Format an error response with the standard schema (ADR-010).
pub fn error_response(code: &str, message: &str) -> Value {
    json!({
        "v": 1,
        "status": "error",
        "code": code,
        "message": message
    })
}
