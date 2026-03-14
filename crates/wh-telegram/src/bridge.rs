//! ZMQ bridge connecting the Telegram surface to the Wheelhouse broker.
//!
//! The bridge uses two ZMQ sockets:
//! - A SUB socket connected to the broker's PUB endpoint (to receive stream messages)
//! - A PUB socket connected to the broker's SUB endpoint (to publish messages)
//!
//! Port convention (per Python SDK `_endpoint_with_port_offset`):
//!   +0 (WH_URL): broker PUB socket — clients subscribe here
//!   +1:          broker SUB socket — clients publish here
//!   +2:          broker control REP — health checks / commands

use prost::Message;
use tracing::{error, instrument};
use zeromq::{PubSocket, Socket, SocketRecv, SocketSend, SubSocket, ZmqMessage};

use wh_proto::{StreamEnvelope, TextMessage};

use crate::error::TelegramError;

/// Sender half of the ZMQ bridge (PUB socket for publishing to stream).
///
/// Obtained via [`ZmqBridge::split()`]. Owns the PUB socket exclusively,
/// so no mutex is needed for concurrent use with [`ZmqSubscriber`].
pub struct ZmqPublisher {
    /// PUB socket connected to broker's SUB endpoint.
    pub_socket: PubSocket,
    /// Stream name for message routing.
    stream_name: String,
    /// Publisher ID for outgoing StreamEnvelopes.
    publisher_id: String,
}

/// Receiver half of the ZMQ bridge (SUB socket for receiving from stream).
///
/// Obtained via [`ZmqBridge::split()`]. Owns the SUB socket exclusively,
/// so no mutex is needed for concurrent use with [`ZmqPublisher`].
pub struct ZmqSubscriber {
    /// SUB socket connected to broker's PUB endpoint.
    sub_socket: SubSocket,
    /// Publisher ID used for self-echo filtering.
    publisher_id: String,
}

impl ZmqPublisher {
    /// Publishes a `TextMessage` to the configured stream.
    ///
    /// Encodes the message into a `StreamEnvelope` and prepends the stream prefix.
    /// `sequence_number` is set to 0 — the broker assigns the authoritative value (FR54).
    ///
    /// Note: `publisher_id` is set at the `StreamEnvelope` level for broker routing.
    /// The `TextMessage.publisher_id` field is a legacy surface-level identifier and
    /// may differ from the envelope-level value. The broker uses the envelope field.
    #[instrument(skip(self, msg))]
    pub async fn publish(&mut self, msg: &TextMessage) -> Result<(), TelegramError> {
        let envelope = StreamEnvelope {
            stream_name: self.stream_name.clone(),
            object_id: String::new(),
            type_url: "wheelhouse.v1.TextMessage".to_string(),
            payload: msg.encode_to_vec(),
            publisher_id: self.publisher_id.clone(),
            published_at_ms: chrono::Utc::now().timestamp_millis(),
            sequence_number: 0, // Broker assigns (FR54)
        };

        let envelope_bytes = envelope.encode_to_vec();

        // Wire format: stream_name\0<StreamEnvelope protobuf bytes>
        let mut wire: Vec<u8> =
            Vec::with_capacity(self.stream_name.len() + 1 + envelope_bytes.len());
        wire.extend_from_slice(self.stream_name.as_bytes());
        wire.push(0);
        wire.extend_from_slice(&envelope_bytes);

        let zmq_msg = ZmqMessage::from(wire);
        self.pub_socket.send(zmq_msg).await.map_err(|e| {
            error!(error = %e, "failed to publish message to stream");
            TelegramError::StreamError(format!("publish failed: {e}"))
        })?;

        tracing::debug!(stream = %self.stream_name, "TextMessage published to stream");
        Ok(())
    }
}

impl ZmqSubscriber {
    /// Receives the next `TextMessage` from the stream.
    ///
    /// Blocks until a message is available. Strips the stream prefix, decodes
    /// the `StreamEnvelope`, and extracts the typed `TextMessage` payload.
    ///
    /// Returns `None` for messages that are not `TextMessage` or cannot be decoded.
    #[instrument(skip(self))]
    pub async fn recv(&mut self) -> Result<Option<(TextMessage, String)>, TelegramError> {
        let msg = self
            .sub_socket
            .recv()
            .await
            .map_err(|e| TelegramError::StreamError(format!("recv failed: {e}")))?;

        let raw: Vec<u8> = msg.try_into().unwrap_or_default();

        // Strip stream prefix: stream_name\0payload
        let Some(null_pos) = raw.iter().position(|&b| b == 0) else {
            tracing::debug!("received message without stream prefix, skipping");
            return Ok(None);
        };

        let payload = &raw[null_pos + 1..];

        // Decode StreamEnvelope
        let envelope = match StreamEnvelope::decode(payload) {
            Ok(env) => env,
            Err(e) => {
                tracing::debug!(error = %e, "failed to decode StreamEnvelope, skipping");
                return Ok(None);
            }
        };

        // Self-echo filter: skip messages we published
        if envelope.publisher_id == self.publisher_id {
            tracing::debug!("self-echo filtered: publisher_id matches our own");
            return Ok(None);
        }

        // Only handle TextMessage type
        if envelope.type_url != "wheelhouse.v1.TextMessage" {
            tracing::debug!(
                type_url = %envelope.type_url,
                "non-TextMessage type received, skipping"
            );
            return Ok(None);
        }

        // Decode TextMessage from payload
        match TextMessage::decode(envelope.payload.as_slice()) {
            Ok(text_msg) => Ok(Some((text_msg, envelope.publisher_id))),
            Err(e) => {
                tracing::debug!(error = %e, "failed to decode TextMessage payload");
                Ok(None)
            }
        }
    }
}

/// ZMQ bridge connecting the Telegram surface to the Wheelhouse broker.
///
/// Manages PUB/SUB sockets for bidirectional communication with the broker.
/// Use `split()` to obtain separate publisher and subscriber handles that
/// can be used concurrently without mutex contention.
pub struct ZmqBridge {
    /// PUB socket connected to broker's SUB endpoint (for publishing to stream).
    pub_socket: PubSocket,
    /// SUB socket connected to broker's PUB endpoint (for receiving from stream).
    sub_socket: SubSocket,
    /// Stream name for message routing.
    stream_name: String,
    /// Publisher ID for outgoing StreamEnvelopes.
    publisher_id: String,
}

impl ZmqBridge {
    /// Connects to the broker via ZMQ PUB/SUB sockets.
    ///
    /// `wh_url` is the broker's PUB endpoint (e.g., `tcp://127.0.0.1:5555`).
    /// The SUB endpoint is derived as PUB port + 1.
    ///
    /// Subscribes to the specified stream name prefix to receive only relevant messages.
    #[instrument(skip_all, fields(wh_url = %wh_url, stream = %stream_name))]
    pub async fn connect(
        wh_url: &str,
        stream_name: &str,
        publisher_id: &str,
    ) -> Result<Self, TelegramError> {
        let sub_endpoint = wh_url.to_string();
        let pub_endpoint = endpoint_with_port_offset(wh_url, 1)?;

        // SUB socket connects to broker PUB endpoint (to receive messages)
        let mut sub_socket = SubSocket::new();
        sub_socket.connect(&sub_endpoint).await.map_err(|e| {
            TelegramError::StreamError(format!("failed to connect SUB socket: {e}"))
        })?;

        // Subscribe to stream prefix (stream_name) to receive only relevant messages
        sub_socket
            .subscribe(stream_name)
            .await
            .map_err(|e| TelegramError::StreamError(format!("failed to subscribe: {e}")))?;

        tracing::info!(
            endpoint = %sub_endpoint,
            stream = %stream_name,
            "SUB socket connected to broker PUB endpoint"
        );

        // PUB socket connects to broker SUB endpoint (to publish messages)
        let mut pub_socket = PubSocket::new();
        pub_socket.connect(&pub_endpoint).await.map_err(|e| {
            TelegramError::StreamError(format!("failed to connect PUB socket: {e}"))
        })?;

        tracing::info!(
            endpoint = %pub_endpoint,
            "PUB socket connected to broker SUB endpoint"
        );

        Ok(Self {
            pub_socket,
            sub_socket,
            stream_name: stream_name.to_string(),
            publisher_id: publisher_id.to_string(),
        })
    }

    /// Splits the bridge into separate publisher and subscriber handles.
    ///
    /// Each half owns its socket exclusively, eliminating the need for a mutex
    /// and allowing truly concurrent publish/subscribe operations from separate
    /// tasks.
    pub fn split(self) -> (ZmqPublisher, ZmqSubscriber) {
        (
            ZmqPublisher {
                pub_socket: self.pub_socket,
                stream_name: self.stream_name,
                publisher_id: self.publisher_id.clone(),
            },
            ZmqSubscriber {
                sub_socket: self.sub_socket,
                publisher_id: self.publisher_id,
            },
        )
    }
}

/// Derives an endpoint with port offset, following the Python SDK convention.
///
/// Examples:
///   `tcp://127.0.0.1:5555` + 1 -> `tcp://127.0.0.1:5556`
///   `tcp://host.containers.internal:5555` + 2 -> `tcp://host.containers.internal:5557`
pub fn endpoint_with_port_offset(endpoint: &str, offset: u16) -> Result<String, TelegramError> {
    // Find the last ':' followed by digits
    if let Some(colon_pos) = endpoint.rfind(':') {
        let port_str = &endpoint[colon_pos + 1..];
        let port: u16 = port_str.parse().map_err(|_| {
            TelegramError::ConfigError(format!(
                "invalid port in endpoint '{endpoint}': '{port_str}'"
            ))
        })?;
        let offset_port = port.checked_add(offset).ok_or_else(|| {
            TelegramError::ConfigError(format!(
                "port overflow: {port} + {offset} exceeds u16 max"
            ))
        })?;
        Ok(format!("{}:{}", &endpoint[..colon_pos], offset_port))
    } else {
        Err(TelegramError::ConfigError(format!(
            "endpoint '{endpoint}' has no port"
        )))
    }
}

/// Encodes a `TextMessage` into `StreamEnvelope` wire bytes (for testing).
///
/// Returns the raw bytes without stream prefix.
pub fn encode_text_message_envelope(
    stream_name: &str,
    publisher_id: &str,
    msg: &TextMessage,
) -> Vec<u8> {
    let envelope = StreamEnvelope {
        stream_name: stream_name.to_string(),
        object_id: String::new(),
        type_url: "wheelhouse.v1.TextMessage".to_string(),
        payload: msg.encode_to_vec(),
        publisher_id: publisher_id.to_string(),
        published_at_ms: chrono::Utc::now().timestamp_millis(),
        sequence_number: 0,
    };
    envelope.encode_to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_with_port_offset_adds_correctly() {
        assert_eq!(
            endpoint_with_port_offset("tcp://127.0.0.1:5555", 1).unwrap(),
            "tcp://127.0.0.1:5556"
        );
        assert_eq!(
            endpoint_with_port_offset("tcp://127.0.0.1:5555", 2).unwrap(),
            "tcp://127.0.0.1:5557"
        );
        assert_eq!(
            endpoint_with_port_offset("tcp://host.containers.internal:5555", 1).unwrap(),
            "tcp://host.containers.internal:5556"
        );
    }

    #[test]
    fn endpoint_with_port_offset_rejects_no_port() {
        assert!(endpoint_with_port_offset("tcp://127.0.0.1", 1).is_err());
    }

    #[test]
    fn endpoint_with_port_offset_rejects_invalid_port() {
        assert!(endpoint_with_port_offset("tcp://127.0.0.1:abc", 1).is_err());
    }

    #[test]
    fn endpoint_with_port_offset_rejects_overflow() {
        assert!(endpoint_with_port_offset("tcp://127.0.0.1:65535", 1).is_err());
    }

    #[test]
    fn publish_encodes_stream_prefix_and_envelope() {
        let msg = TextMessage {
            content: "Hello".to_string(),
            publisher_id: "telegram-surface".to_string(),
            timestamp_ms: 1000,
            user_id: "usr_abc".to_string(),
            reply_to_user_id: String::new(),
        };

        let envelope_bytes = encode_text_message_envelope("main", "telegram-surface", &msg);

        // Verify envelope decodes correctly
        let envelope = StreamEnvelope::decode(envelope_bytes.as_slice()).unwrap();
        assert_eq!(envelope.type_url, "wheelhouse.v1.TextMessage");
        assert_eq!(envelope.publisher_id, "telegram-surface");
        assert_eq!(envelope.stream_name, "main");
        assert_eq!(envelope.sequence_number, 0); // Never set by publisher (FR54)

        // Verify inner TextMessage decodes
        let decoded_msg = TextMessage::decode(envelope.payload.as_slice()).unwrap();
        assert_eq!(decoded_msg.content, "Hello");
        assert_eq!(decoded_msg.user_id, "usr_abc");
    }

    #[test]
    fn recv_decode_roundtrip() {
        let msg = TextMessage {
            content: "Test".to_string(),
            publisher_id: "telegram-surface".to_string(),
            timestamp_ms: 2000,
            user_id: "usr_123".to_string(),
            reply_to_user_id: "usr_456".to_string(),
        };

        let envelope_bytes = encode_text_message_envelope("main", "agent-donna", &msg);

        // Simulate wire format: stream_name\0envelope_bytes
        let mut wire: Vec<u8> = Vec::new();
        wire.extend_from_slice(b"main");
        wire.push(0);
        wire.extend_from_slice(&envelope_bytes);

        // Decode the wire format
        let null_pos = wire.iter().position(|&b| b == 0).unwrap();
        let payload = &wire[null_pos + 1..];
        let envelope = StreamEnvelope::decode(payload).unwrap();
        assert_eq!(envelope.type_url, "wheelhouse.v1.TextMessage");

        let decoded = TextMessage::decode(envelope.payload.as_slice()).unwrap();
        assert_eq!(decoded.content, "Test");
        assert_eq!(decoded.reply_to_user_id, "usr_456");
    }
}
