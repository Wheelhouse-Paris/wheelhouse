//! Broker routing loop (ADR-009).
//!
//! Single async routing loop with `tokio::select! { biased; }`.
//! Shutdown via `CancellationToken` -- checked before recv (SC-06).
//! WAL write-before-route enforced by `WalReceipt` token (5W-01, FM-01).
//! StreamEnvelope decode/augment/re-encode for typed publish/subscribe (Story 1.4).

use std::sync::atomic::Ordering;
use std::sync::Arc;

use prost::Message;
use tokio_util::sync::CancellationToken;
use wh_proto::StreamEnvelope;
use zeromq::{PubSocket, Socket, SocketRecv, SocketSend, SubSocket, ZmqMessage};

use crate::config::BrokerConfig;
use crate::error::BrokerError;
use crate::metrics::BrokerState;
use crate::skill_router;

/// Run the routing loop (ADR-009, SC-06).
///
/// Binds PUB and SUB sockets and forwards messages from SUB to PUB.
/// Messages for known streams are persisted to WAL before forwarding (5W-01).
/// StreamEnvelope payloads are augmented with broker-assigned sequence numbers (FR54).
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
///
/// Story 1.4: If payload decodes as StreamEnvelope, the broker persists the raw
/// envelope to WAL first, then on success assigns an authoritative sequence_number
/// and published_at_ms, re-encodes, and forwards. Sequence numbers are only consumed
/// after successful WAL write to prevent gaps on WAL failure.
/// If decode fails, raw bytes are forwarded as-is (backward compat).
async fn route_message(msg: ZmqMessage, pub_socket: &mut PubSocket, state: &Arc<BrokerState>) {
    let raw: Vec<u8> = msg.try_into().unwrap_or_default();

    // Extract stream name from message: convention is "stream_name\0payload"
    let Some(null_pos) = raw.iter().position(|&b| b == 0) else {
        // No stream prefix — forward without WAL (backward compat)
        let forward_msg = ZmqMessage::from(raw);
        if let Err(e) = pub_socket.send(forward_msg).await {
            tracing::warn!(error = %e, "failed to forward message to PUB socket");
        }
        return;
    };

    let stream_name = String::from_utf8_lossy(&raw[..null_pos]).to_string();
    let payload = &raw[null_pos + 1..];

    let streams = state.streams.read().await;
    let Some(stream_info) = streams.get(&stream_name) else {
        // Unknown stream — forward without WAL (backward compat)
        drop(streams);
        let forward_msg = ZmqMessage::from(raw);
        if let Err(e) = pub_socket.send(forward_msg).await {
            tracing::warn!(error = %e, "failed to forward message to PUB socket");
        }
        return;
    };

    // Attempt to decode payload as StreamEnvelope for typed message handling.
    // WAL persists raw payload first; sequence number assigned only after WAL success.
    let decoded_envelope = StreamEnvelope::decode(payload).ok();

    // Stream exists — WAL write before forward (5W-01, FM-01)
    match stream_info.wal_writer.write(payload).await {
        Ok(receipt) => {
            // WAL write succeeded — acknowledge receipt
            stream_info.message_count.fetch_add(1, Ordering::Relaxed);
            receipt.acknowledge();

            // Now assign sequence number — only after successful WAL write (FR54)
            let forward_bytes = if let Some(mut envelope) = decoded_envelope {
                let seq = stream_info.sequence_counter.fetch_add(1, Ordering::Relaxed);
                envelope.sequence_number = seq;
                envelope.published_at_ms = chrono::Utc::now().timestamp_millis();
                envelope.encode_to_vec()
            } else {
                payload.to_vec()
            };

            // Drop streams lock before async skill execution to avoid blocking
            // stream create/delete operations during skill processing.
            drop(streams);

            // Check for SkillInvocation interception (Story 9.3)
            // Decode the forwarded envelope to check type_url
            let skill_responses = if let Some(ref skill_router) = state.skill_router {
                if let Ok(envelope) = StreamEnvelope::decode(forward_bytes.as_slice()) {
                    if envelope.type_url == skill_router::TYPE_URL_SKILL_INVOCATION {
                        if let Ok(invocation) =
                            wh_proto::SkillInvocation::decode(envelope.payload.as_slice())
                        {
                            let request =
                                wh_skill::invocation::SkillInvocationRequest::from(invocation);
                            Some(skill_router.handle_invocation(request).await)
                        } else {
                            tracing::warn!(
                                stream = %stream_name,
                                "failed to decode SkillInvocation payload"
                            );
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            // Build the forwarded ZMQ message: stream_name\0augmented_payload
            let mut wire: Vec<u8> = Vec::with_capacity(stream_name.len() + 1 + forward_bytes.len());
            wire.extend_from_slice(stream_name.as_bytes());
            wire.push(0);
            wire.extend_from_slice(&forward_bytes);

            let forward_msg = ZmqMessage::from(wire);
            if let Err(e) = pub_socket.send(forward_msg).await {
                tracing::warn!(error = %e, "failed to forward message to PUB socket");
            }

            // Publish skill responses back to the stream (Story 9.3)
            if let Some(responses) = skill_responses {
                for response in responses {
                    let mut response_envelope =
                        skill_router::build_response_envelope(&stream_name, &response);

                    // WAL write the skill response, then assign sequence + publish
                    let streams = state.streams.read().await;
                    if let Some(stream_info) = streams.get(&stream_name) {
                        let response_payload = response_envelope.encode_to_vec();
                        match stream_info.wal_writer.write(&response_payload).await {
                            Ok(receipt) => {
                                stream_info.message_count.fetch_add(1, Ordering::Relaxed);
                                receipt.acknowledge();

                                // Assign sequence number
                                let seq =
                                    stream_info.sequence_counter.fetch_add(1, Ordering::Relaxed);
                                response_envelope.sequence_number = seq;
                                response_envelope.published_at_ms =
                                    chrono::Utc::now().timestamp_millis();

                                let final_payload = response_envelope.encode_to_vec();
                                drop(streams);

                                let mut response_wire: Vec<u8> =
                                    Vec::with_capacity(stream_name.len() + 1 + final_payload.len());
                                response_wire.extend_from_slice(stream_name.as_bytes());
                                response_wire.push(0);
                                response_wire.extend_from_slice(&final_payload);

                                let response_msg = ZmqMessage::from(response_wire);
                                if let Err(e) = pub_socket.send(response_msg).await {
                                    tracing::warn!(
                                        error = %e,
                                        "failed to publish skill response to PUB socket"
                                    );
                                }
                            }
                            Err(e) => {
                                drop(streams);
                                tracing::warn!(
                                    stream = %stream_name,
                                    error = %e,
                                    "WAL write failed for skill response — not published (SC-07)"
                                );
                            }
                        }
                    } else {
                        drop(streams);
                    }
                }
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
}
