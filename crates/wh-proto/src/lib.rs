//! Wheelhouse protocol types and stream envelope support.
//!
//! This crate provides the stream envelope type that carries custom Protobuf types
//! through the ZMQ data plane. The envelope includes the fully-qualified type name
//! so receivers can deserialize known types or fall back to raw bytes.

use serde::{Deserialize, Serialize};

/// Stream envelope for carrying typed messages through the ZMQ data plane.
///
/// Per SC-01, the ZMQ data plane uses a 3-frame envelope:
/// - Frame 0: topic/stream name
/// - Frame 1: type name (fully-qualified: `<namespace>.<TypeName>`)
/// - Frame 2: payload bytes (serialized protobuf or raw bytes)
///
/// This struct represents the logical envelope — ZMQ framing is handled at the transport layer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StreamEnvelope {
    /// Stream/topic name
    pub stream_name: String,
    /// Fully-qualified type name: `<namespace>.<TypeName>`
    pub type_name: String,
    /// Serialized payload bytes
    pub payload: Vec<u8>,
}

impl StreamEnvelope {
    /// Create a new stream envelope.
    pub fn new(stream_name: impl Into<String>, type_name: impl Into<String>, payload: Vec<u8>) -> Self {
        Self {
            stream_name: stream_name.into(),
            type_name: type_name.into(),
            payload,
        }
    }

    /// Serialize envelope to ZMQ-compatible frames (3-frame format per SC-01).
    /// Returns (stream_name_bytes, type_name_bytes, payload_bytes).
    pub fn to_frames(&self) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        (
            self.stream_name.as_bytes().to_vec(),
            self.type_name.as_bytes().to_vec(),
            self.payload.clone(),
        )
    }

    /// Deserialize envelope from ZMQ frames.
    /// Returns `None` if frames are invalid UTF-8 for stream_name or type_name.
    pub fn from_frames(
        stream_name_bytes: &[u8],
        type_name_bytes: &[u8],
        payload_bytes: &[u8],
    ) -> Option<Self> {
        let stream_name = std::str::from_utf8(stream_name_bytes).ok()?.to_string();
        let type_name = std::str::from_utf8(type_name_bytes).ok()?.to_string();
        Some(Self {
            stream_name,
            type_name,
            payload: payload_bytes.to_vec(),
        })
    }

    /// Check if this is a core wheelhouse type.
    pub fn is_core_type(&self) -> bool {
        self.type_name.starts_with("wheelhouse.")
    }
}

/// Result of attempting to receive and deserialize a typed message.
///
/// Per AC #2: if the receiver knows the type, it gets deserialized data.
/// If the receiver does not know the type, it gets raw bytes + type name.
/// Never a silent failure or crash.
#[derive(Debug, Clone)]
pub enum TypedMessage {
    /// Type was known and deserialized successfully.
    Known {
        type_name: String,
        /// Deserialized data — generic bytes that the application layer interprets.
        data: Vec<u8>,
    },
    /// Type was unknown — raw bytes returned with type name for inspection.
    Unknown {
        type_name: String,
        raw_bytes: Vec<u8>,
    },
}

impl TypedMessage {
    /// Get the type name regardless of known/unknown status.
    pub fn type_name(&self) -> &str {
        match self {
            TypedMessage::Known { type_name, .. } => type_name,
            TypedMessage::Unknown { type_name, .. } => type_name,
        }
    }

    /// Check if the type was known.
    pub fn is_known(&self) -> bool {
        matches!(self, TypedMessage::Known { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_envelope_round_trip() {
        let envelope = StreamEnvelope::new(
            "main",
            "biotech.MoleculeObject",
            vec![1, 2, 3, 4],
        );

        let (f0, f1, f2) = envelope.to_frames();
        let restored = StreamEnvelope::from_frames(&f0, &f1, &f2).unwrap();

        assert_eq!(envelope, restored);
    }

    #[test]
    fn stream_envelope_carries_custom_type_name() {
        let envelope = StreamEnvelope::new(
            "main",
            "biotech.MoleculeObject",
            vec![10, 20, 30],
        );

        assert_eq!(envelope.type_name, "biotech.MoleculeObject");
        assert_eq!(envelope.stream_name, "main");
        assert_eq!(envelope.payload, vec![10, 20, 30]);
    }

    #[test]
    fn stream_envelope_detects_core_type() {
        let core = StreamEnvelope::new("main", "wheelhouse.TextMessage", vec![]);
        assert!(core.is_core_type());

        let custom = StreamEnvelope::new("main", "biotech.Molecule", vec![]);
        assert!(!custom.is_core_type());
    }

    #[test]
    fn envelope_from_invalid_utf8_returns_none() {
        let result = StreamEnvelope::from_frames(
            b"main",
            &[0xFF, 0xFE], // Invalid UTF-8
            b"payload",
        );
        assert!(result.is_none());
    }

    #[test]
    fn typed_message_unknown_type_returns_raw_bytes() {
        let msg = TypedMessage::Unknown {
            type_name: "biotech.MoleculeObject".to_string(),
            raw_bytes: vec![1, 2, 3],
        };
        assert!(!msg.is_known());
        assert_eq!(msg.type_name(), "biotech.MoleculeObject");
        match msg {
            TypedMessage::Unknown {
                type_name,
                raw_bytes,
            } => {
                assert_eq!(type_name, "biotech.MoleculeObject");
                assert_eq!(raw_bytes, vec![1, 2, 3]);
            }
            _ => panic!("Expected Unknown variant"),
        }
    }

    #[test]
    fn typed_message_known_type() {
        let msg = TypedMessage::Known {
            type_name: "biotech.MoleculeObject".to_string(),
            data: vec![1, 2, 3],
        };
        assert!(msg.is_known());
        assert_eq!(msg.type_name(), "biotech.MoleculeObject");
    }
}
