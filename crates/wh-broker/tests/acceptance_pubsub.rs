//! Acceptance tests for Story 1.4: Typed Protobuf Publish and Subscribe with Delivery Order.
//!
//! Tests exercise publish/subscribe through ZMQ PUB/SUB sockets with typed StreamEnvelope
//! wrapping, delivery order guarantees, and fan-out to multiple subscribers.
//!
//! TDD Red Phase: These tests are expected to FAIL until implementation is complete.

use std::sync::Arc;
use std::time::Duration;

use prost::Message;
use wh_broker::config::BrokerConfig;
use wh_broker::metrics::BrokerState;
use wh_proto::{StreamEnvelope, TextMessage};
use zeromq::{PubSocket, Socket, SocketRecv, SocketSend, SubSocket, ZmqMessage};

/// Helper: start a broker (routing loop + control loop) in the background and return
/// the config, state, and a cancellation token.
async fn start_test_broker() -> (
    BrokerConfig,
    Arc<BrokerState>,
    tokio_util::sync::CancellationToken,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let pub_port = portpicker::pick_unused_port().unwrap();
    let sub_port = portpicker::pick_unused_port().unwrap();
    let control_port = portpicker::pick_unused_port().unwrap();

    let config = BrokerConfig::with_ports_and_data_dir(
        pub_port,
        sub_port,
        control_port,
        dir.path().to_path_buf(),
    );
    let state = BrokerState::with_data_dir(config.data_dir().to_path_buf());
    let cancel = tokio_util::sync::CancellationToken::new();

    // Create test stream
    state
        .create_stream("main", Some(Duration::from_secs(86400)), None)
        .await
        .unwrap();

    // Start routing loop
    let routing_config = config.clone();
    let routing_state = Arc::clone(&state);
    let routing_cancel = cancel.clone();
    tokio::spawn(async move {
        wh_broker::routing::run_routing_loop(&routing_config, routing_state, routing_cancel)
            .await
            .ok();
    });

    // Give the routing loop time to bind sockets
    tokio::time::sleep(Duration::from_millis(200)).await;

    (config, state, cancel, dir)
}

/// Helper: create a publisher socket connected to the broker's SUB endpoint.
async fn create_publisher(config: &BrokerConfig) -> PubSocket {
    let mut pub_socket = PubSocket::new();
    pub_socket
        .connect(config.sub_endpoint().as_str())
        .await
        .unwrap();
    // PUB sockets need time to establish connection
    tokio::time::sleep(Duration::from_millis(100)).await;
    pub_socket
}

/// Helper: create a subscriber socket connected to the broker's PUB endpoint.
async fn create_subscriber(config: &BrokerConfig, stream_name: &str) -> SubSocket {
    let mut sub_socket = SubSocket::new();
    sub_socket
        .connect(config.pub_endpoint().as_str())
        .await
        .unwrap();
    // Subscribe to stream topic prefix
    let topic = format!("{stream_name}\0");
    sub_socket.subscribe(&topic).await.unwrap();
    // SUB sockets need time to propagate subscription
    tokio::time::sleep(Duration::from_millis(200)).await;
    sub_socket
}

/// Helper: build a StreamEnvelope wrapping a TextMessage.
fn build_text_envelope(stream: &str, content: &str, publisher: &str) -> Vec<u8> {
    let text_msg = TextMessage {
        content: content.to_string(),
        publisher_id: publisher.to_string(),
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        user_id: String::new(),
        reply_to_user_id: String::new(),
        source_stream: String::new(),
        source_topic: String::new(),
    };
    let envelope = StreamEnvelope {
        stream_name: stream.to_string(),
        object_id: format!("test-{}", uuid::Uuid::new_v4()),
        type_url: "wheelhouse.v1.TextMessage".to_string(),
        payload: text_msg.encode_to_vec(),
        publisher_id: publisher.to_string(),
        published_at_ms: chrono::Utc::now().timestamp_millis(),
        sequence_number: 0, // Broker assigns the real sequence number
    };
    envelope.encode_to_vec()
}

/// Helper: publish a message to a stream via ZMQ.
async fn publish_to_stream(pub_socket: &mut PubSocket, stream_name: &str, envelope_bytes: &[u8]) {
    let mut wire: Vec<u8> = Vec::new();
    wire.extend_from_slice(stream_name.as_bytes());
    wire.push(0); // null separator
    wire.extend_from_slice(envelope_bytes);
    let msg = ZmqMessage::from(wire);
    pub_socket.send(msg).await.unwrap();
}

/// Helper: receive a message and decode it as StreamEnvelope.
async fn receive_envelope(sub_socket: &mut SubSocket, timeout: Duration) -> Option<StreamEnvelope> {
    match tokio::time::timeout(timeout, sub_socket.recv()).await {
        Ok(Ok(msg)) => {
            let raw: Vec<u8> = msg.try_into().unwrap_or_default();
            // Strip stream_name\0 prefix
            if let Some(null_pos) = raw.iter().position(|&b| b == 0) {
                let payload = &raw[null_pos + 1..];
                StreamEnvelope::decode(payload).ok()
            } else {
                None
            }
        }
        _ => None,
    }
}

// ─── AC#1: Publish TextMessage and subscriber receives it ───

#[tokio::test]
async fn test_publish_text_message_received_by_subscriber() {
    let (config, _state, cancel, _dir) = start_test_broker().await;

    let mut publisher = create_publisher(&config).await;
    let mut subscriber = create_subscriber(&config, "main").await;

    let envelope_bytes = build_text_envelope("main", "hello", "test-pub");
    publish_to_stream(&mut publisher, "main", &envelope_bytes).await;

    let received = receive_envelope(&mut subscriber, Duration::from_secs(5)).await;
    assert!(received.is_some(), "Subscriber should receive the message");

    let envelope = received.unwrap();
    assert_eq!(envelope.type_url, "wheelhouse.v1.TextMessage");

    // Decode inner TextMessage
    let text_msg = TextMessage::decode(envelope.payload.as_slice()).unwrap();
    assert_eq!(text_msg.content, "hello");

    cancel.cancel();
}

// ─── AC#2: 100 messages arrive in exact order (FR54) ───

#[tokio::test]
async fn test_delivery_order_100_messages() {
    let (config, _state, cancel, _dir) = start_test_broker().await;

    let mut publisher = create_publisher(&config).await;
    let mut subscriber = create_subscriber(&config, "main").await;

    // Publish 100 messages sequentially
    for i in 0..100 {
        let envelope_bytes = build_text_envelope("main", &format!("msg-{i}"), "order-test");
        publish_to_stream(&mut publisher, "main", &envelope_bytes).await;
    }

    // Receive all 100 and verify order
    let mut received_contents = Vec::new();
    for _ in 0..100 {
        let envelope = receive_envelope(&mut subscriber, Duration::from_secs(5))
            .await
            .expect("Should receive message");
        let text_msg = TextMessage::decode(envelope.payload.as_slice()).unwrap();
        received_contents.push(text_msg.content);
    }

    for (i, content) in received_contents.iter().enumerate() {
        assert_eq!(
            content,
            &format!("msg-{i}"),
            "Message {i} out of order: expected msg-{i}, got {content}"
        );
    }

    cancel.cancel();
}

// ─── AC#3: Fan-out to multiple subscribers ───

#[tokio::test]
async fn test_fan_out_multiple_subscribers() {
    let (config, _state, cancel, _dir) = start_test_broker().await;

    let mut publisher = create_publisher(&config).await;
    let mut sub1 = create_subscriber(&config, "main").await;
    let mut sub2 = create_subscriber(&config, "main").await;

    let envelope_bytes = build_text_envelope("main", "broadcast", "fan-out-test");
    publish_to_stream(&mut publisher, "main", &envelope_bytes).await;

    let recv1 = receive_envelope(&mut sub1, Duration::from_secs(5)).await;
    let recv2 = receive_envelope(&mut sub2, Duration::from_secs(5)).await;

    assert!(recv1.is_some(), "Subscriber 1 should receive the message");
    assert!(recv2.is_some(), "Subscriber 2 should receive the message");

    let text1 = TextMessage::decode(recv1.unwrap().payload.as_slice()).unwrap();
    let text2 = TextMessage::decode(recv2.unwrap().payload.as_slice()).unwrap();
    assert_eq!(text1.content, "broadcast");
    assert_eq!(text2.content, "broadcast");

    cancel.cancel();
}

// ─── AC#4: Agent-to-agent typed Protobuf deserialization ───

#[tokio::test]
async fn test_typed_protobuf_deserialization_agent_to_agent() {
    let (config, _state, cancel, _dir) = start_test_broker().await;

    let mut publisher = create_publisher(&config).await;
    let mut subscriber = create_subscriber(&config, "main").await;

    let envelope_bytes = build_text_envelope("main", "agent communication", "agent-1");
    publish_to_stream(&mut publisher, "main", &envelope_bytes).await;

    let received = receive_envelope(&mut subscriber, Duration::from_secs(5)).await;
    assert!(
        received.is_some(),
        "Agent subscriber should receive message"
    );

    let envelope = received.unwrap();
    assert_eq!(envelope.type_url, "wheelhouse.v1.TextMessage");
    assert!(!envelope.payload.is_empty());

    // Verify the receiving agent can fully deserialize the Protobuf payload
    let text_msg = TextMessage::decode(envelope.payload.as_slice()).unwrap();
    assert_eq!(text_msg.content, "agent communication");
    assert_eq!(text_msg.publisher_id, "agent-1");

    cancel.cancel();
}

// ─── Sequence number assignment ───

#[tokio::test]
async fn test_sequence_numbers_monotonically_increasing() {
    let (config, _state, cancel, _dir) = start_test_broker().await;

    let mut publisher = create_publisher(&config).await;
    let mut subscriber = create_subscriber(&config, "main").await;

    // Publish 10 messages
    for i in 0..10 {
        let envelope_bytes = build_text_envelope("main", &format!("seq-{i}"), "seq-test");
        publish_to_stream(&mut publisher, "main", &envelope_bytes).await;
    }

    // Verify sequence numbers are monotonically increasing
    let mut prev_seq = 0u64;
    for i in 0..10 {
        let envelope = receive_envelope(&mut subscriber, Duration::from_secs(5))
            .await
            .expect("Should receive message");
        if i > 0 {
            assert!(
                envelope.sequence_number > prev_seq,
                "Sequence number should be monotonically increasing: {} <= {}",
                envelope.sequence_number,
                prev_seq
            );
        }
        prev_seq = envelope.sequence_number;
    }

    cancel.cancel();
}

// ─── Backward compatibility: raw messages forwarded ───

#[tokio::test]
async fn test_raw_message_forwarded_without_envelope() {
    let (config, _state, cancel, _dir) = start_test_broker().await;

    let mut publisher = create_publisher(&config).await;
    let mut sub_socket = SubSocket::new();
    sub_socket
        .connect(config.pub_endpoint().as_str())
        .await
        .unwrap();
    sub_socket.subscribe("main\0").await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Send a raw (non-envelope) message
    let mut wire: Vec<u8> = Vec::new();
    wire.extend_from_slice(b"main");
    wire.push(0);
    wire.extend_from_slice(b"raw payload data");
    let msg = ZmqMessage::from(wire);
    publisher.send(msg).await.unwrap();

    // Should still be forwarded
    let result = tokio::time::timeout(Duration::from_secs(5), sub_socket.recv()).await;
    assert!(result.is_ok(), "Raw message should still be forwarded");

    cancel.cancel();
}
