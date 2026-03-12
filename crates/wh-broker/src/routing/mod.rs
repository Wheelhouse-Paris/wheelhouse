//! Broker routing loop skeleton (ADR-009).
//!
//! Single async routing loop with `tokio::select! { biased; }`.
//! Shutdown via `CancellationToken` -- checked before recv (SC-06).
//! For Story 1.2, this is a skeleton that forwards SUB to PUB without WAL.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use zeromq::{PubSocket, Socket, SocketRecv, SocketSend, SubSocket, ZmqMessage};

use crate::config::BrokerConfig;
use crate::error::BrokerError;
use crate::metrics::BrokerState;

/// Run the routing loop (ADR-009, SC-06).
///
/// Binds PUB and SUB sockets and forwards messages from SUB to PUB.
/// In this story (1.2), this is a minimal skeleton -- no WAL, no stream management.
#[tracing::instrument(skip_all)]
pub async fn run_routing_loop(
    config: &BrokerConfig,
    _state: Arc<BrokerState>,
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
                        route_message(msg, &mut pub_socket).await;
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

/// Route a single message from SUB to PUB (PP-04).
///
/// Extracted as a separate async fn per architecture requirement.
/// In Story 1.2, this is a simple forward. WAL write-before-route (5W-01)
/// will be added in Story 1.3+.
async fn route_message(msg: ZmqMessage, pub_socket: &mut PubSocket) {
    if let Err(e) = pub_socket.send(msg).await {
        tracing::warn!(error = %e, "failed to forward message to PUB socket");
    }
}
