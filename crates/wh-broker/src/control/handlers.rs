//! Control socket command dispatch table (ADR-010).
//!
//! Each handler returns a JSON response with `"v": 1` schema version.
//! Dispatch table kept minimal (~30 lines max per architecture).

use std::sync::Arc;

use serde_json::{json, Value};

use crate::error::ControlError;
use crate::metrics::{
    format_retention_duration, parse_retention_duration, parse_retention_size, BrokerState,
    StreamError,
};

/// Dispatch a control command to the appropriate handler.
///
/// Returns the JSON response value to send back to the client.
/// The full request payload is passed so handlers can extract additional fields.
pub async fn dispatch(
    command: &str,
    payload: &Value,
    state: &Arc<BrokerState>,
) -> Result<Value, ControlError> {
    match command {
        "status" => handle_status(state).await,
        "stream_create" => handle_stream_create(payload, state).await,
        "stream_list" => handle_stream_list(state).await,
        "stream_delete" => handle_stream_delete(payload, state).await,
        other => Err(ControlError::UnknownCommand(other.to_string())),
    }
}

/// Handle the `status` command -- returns broker health data (AC#2).
///
/// Response matches canonical `wh status` JSON schema (SCV-02).
/// Now includes stream data from the stream registry.
#[tracing::instrument(skip_all)]
async fn handle_status(state: &Arc<BrokerState>) -> Result<Value, ControlError> {
    let uptime_secs = state.metrics.uptime_secs();
    let panic_count = state.metrics.get_panic_count();
    let subscriber_count = *state.subscriber_count.read().await;

    let stream_list = state.list_streams().await;
    let streams_json: Vec<Value> = {
        let streams = state.streams.read().await;
        stream_list
            .iter()
            .map(|meta| {
                let message_count = streams
                    .get(&meta.name)
                    .map(|s| s.message_count.load(std::sync::atomic::Ordering::Relaxed))
                    .unwrap_or(0);
                json!({
                    "name": meta.name,
                    "message_count": message_count,
                    "subscriber_count": 0
                })
            })
            .collect()
    };

    Ok(json!({
        "v": 1,
        "status": "ok",
        "data": {
            "uptime_secs": uptime_secs,
            "panic_count": panic_count,
            "subscriber_count": subscriber_count,
            "streams": streams_json
        }
    }))
}

/// Handle `stream_create` command.
#[tracing::instrument(skip_all)]
async fn handle_stream_create(
    payload: &Value,
    state: &Arc<BrokerState>,
) -> Result<Value, ControlError> {
    let name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ControlError::Internal("Missing 'name' field".to_string()))?;

    let retention_duration = payload
        .get("retention")
        .and_then(|v| v.as_str())
        .map(parse_retention_duration)
        .transpose()
        .map_err(ControlError::Internal)?;

    let retention_size = payload
        .get("retention_size")
        .and_then(|v| v.as_str())
        .map(parse_retention_size)
        .transpose()
        .map_err(ControlError::Internal)?;

    match state
        .create_stream(name, retention_duration, retention_size)
        .await
    {
        Ok(()) => {
            let retention_str = retention_duration
                .map(|d| format_retention_duration(&d))
                .unwrap_or_else(|| "none".to_string());
            Ok(json!({
                "v": 1,
                "status": "ok",
                "data": {
                    "name": name,
                    "retention": retention_str
                }
            }))
        }
        Err(StreamError::AlreadyExists(n)) => Ok(error_response(
            "STREAM_EXISTS",
            &format!("Stream '{n}' already exists"),
        )),
        Err(StreamError::InvalidName(msg)) => Ok(error_response("INVALID_STREAM_NAME", &msg)),
        Err(e) => Ok(error_response("INTERNAL_ERROR", &e.to_string())),
    }
}

/// Handle `stream_list` command.
#[tracing::instrument(skip_all)]
async fn handle_stream_list(state: &Arc<BrokerState>) -> Result<Value, ControlError> {
    let stream_list = state.list_streams().await;
    let streams = state.streams.read().await;

    let streams_json: Vec<Value> = stream_list
        .iter()
        .map(|meta| {
            let message_count = streams
                .get(&meta.name)
                .map(|s| s.message_count.load(std::sync::atomic::Ordering::Relaxed))
                .unwrap_or(0);
            let retention = meta
                .retention_secs
                .map(|s| format_retention_duration(&std::time::Duration::from_secs(s)));
            let created_at = chrono::DateTime::from_timestamp_millis(meta.created_at_epoch_ms)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();

            json!({
                "name": meta.name,
                "retention": retention,
                "retention_size_bytes": meta.retention_size_bytes,
                "message_count": message_count,
                "created_at": created_at
            })
        })
        .collect();

    Ok(json!({
        "v": 1,
        "status": "ok",
        "data": {
            "streams": streams_json
        }
    }))
}

/// Handle `stream_delete` command.
#[tracing::instrument(skip_all)]
async fn handle_stream_delete(
    payload: &Value,
    state: &Arc<BrokerState>,
) -> Result<Value, ControlError> {
    let name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ControlError::Internal("Missing 'name' field".to_string()))?;

    match state.delete_stream(name).await {
        Ok(()) => Ok(json!({
            "v": 1,
            "status": "ok",
            "data": {
                "name": name
            }
        })),
        Err(StreamError::NotFound(n)) => Ok(error_response(
            "STREAM_NOT_FOUND",
            &format!("Stream '{n}' not found"),
        )),
        Err(e) => Ok(error_response("INTERNAL_ERROR", &e.to_string())),
    }
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
