//! Broker routing loop (ADR-009).
//!
//! Single async routing loop with `tokio::select! { biased; }`.
//! Shutdown via `CancellationToken` -- checked before recv (SC-06).
//! WAL write-before-route enforced by `WalReceipt` token (5W-01, FM-01).

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use zeromq::{PubSocket, Socket, SocketRecv, SocketSend, SubSocket, ZmqMessage};

use crate::config::BrokerConfig;
use crate::error::BrokerError;
use crate::metrics::BrokerState;

/// Run the routing loop (ADR-009, SC-06).
///
/// Binds PUB and SUB sockets and forwards messages from SUB to PUB.
/// Messages for known streams are persisted to WAL before forwarding (5W-01).
#[tracing::instrument(skip_all)]
pub async fn run_routing_loop(
    config: &BrokerConfig,
    state: Arc<BrokerState>,
    cancel: CancellationToken,
) -> Result<(), BrokerError> {
    let mut pub_socket = PubSocket::new();
    pub_socket
        .bind(config.pub_endpoint().as_str())
        .await
        .map_err(|e| BrokerError::BindError {
            endpoint: config.pub_endpoint(),
            source: e,
        })?;

    tracing::info!(
        endpoint = %config.pub_endpoint(),
        "PUB socket bound on 127.0.0.1"
    );

    let mut sub_socket = SubSocket::new();
    sub_socket
        .bind(config.sub_endpoint().as_str())
        .await
        .map_err(|e| BrokerError::BindError {
            endpoint: config.sub_endpoint(),
            source: e,
        })?;

    // Subscribe to all messages
    sub_socket
        .subscribe("")
        .await
        .map_err(|e| BrokerError::RoutingError(format!("Failed to subscribe: {e}")))?;

    tracing::info!(
        endpoint = %config.sub_endpoint(),
        "SUB socket bound on 127.0.0.1"
    );

    loop {
        tokio::select! {
            biased;

            // Shutdown signal checked before recv (SC-06)
            _ = cancel.cancelled() => {
                tracing::info!("routing loop shutting down");
                break;
            }

            // Receive from SUB and forward to PUB
            result = sub_socket.recv() => {
                match result {
                    Ok(msg) => {
                        route_message(msg, &mut pub_socket, &state).await;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "routing loop recv error");
                        // Yield on error path to prevent CPU spin (SC-09)
                        tokio::task::yield_now().await;
                    }
                }
            }
        }
    }

    Ok(())
}

/// Route a single message from SUB to PUB (PP-04, 5W-01, FM-01).
///
/// Extracted as a separate async fn per architecture requirement.
/// If the message has a stream prefix (stream_name\0payload), attempt WAL write first.
/// WAL write must complete before forwarding (5W-01).
/// If WAL write fails, message is NOT forwarded (SC-07).
async fn route_message(msg: ZmqMessage, pub_socket: &mut PubSocket, state: &Arc<BrokerState>) {
    let raw: Vec<u8> = msg.clone().try_into().unwrap_or_default();

    // Extract stream name from message: convention is "stream_name\0payload"
    if let Some(null_pos) = raw.iter().position(|&b| b == 0) {
        let stream_name = String::from_utf8_lossy(&raw[..null_pos]).to_string();
        let payload = &raw[null_pos + 1..];

        let streams = state.streams.read().await;
        if let Some(stream_info) = streams.get(&stream_name) {
            // Stream exists — WAL write before forward (5W-01, FM-01)
            match stream_info.wal_writer.write(payload).await {
                Ok(receipt) => {
                    // WAL write succeeded — acknowledge receipt and forward
                    stream_info.message_count.fetch_add(1, Ordering::Relaxed);
                    receipt.acknowledge();

                    drop(streams);
                    if let Err(e) = pub_socket.send(msg).await {
                        tracing::warn!(error = %e, "failed to forward message to PUB socket");
                    }
                }
                Err(e) => {
                    // WAL write failed — do NOT forward (SC-07)
                    tracing::warn!(
                        stream = %stream_name,
                        error = %e,
                        "WAL write failed — message not forwarded (SC-07)"
                    );
                }
            }
        } else {
            // Unknown stream — forward without WAL (backward compat)
            drop(streams);
            if let Err(e) = pub_socket.send(msg).await {
                tracing::warn!(error = %e, "failed to forward message to PUB socket");
            }
        }
    } else {
        // No stream prefix — forward without WAL (backward compat)
        if let Err(e) = pub_socket.send(msg).await {
            tracing::warn!(error = %e, "failed to forward message to PUB socket");
        }
    }
}
