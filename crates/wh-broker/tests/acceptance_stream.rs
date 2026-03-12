//! Acceptance tests for Story 1.3: Stream Create/List/Delete and WAL Persistence.
//!
//! Tests exercise the stream registry, WAL persistence, and control socket handlers.

use std::time::Duration;

use wh_broker::metrics::BrokerState;
use wh_broker::wal::WalWriter;

// ─── AC#1: Stream Create and List ───

#[tokio::test]
async fn test_stream_create_appears_in_list() {
    let dir = tempfile::tempdir().unwrap();
    let state = BrokerState::with_data_dir(dir.path().to_path_buf());

    state
        .create_stream("main", Some(Duration::from_secs(7 * 86400)), None)
        .await
        .unwrap();

    let streams = state.list_streams().await;
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].name, "main");
    assert_eq!(streams[0].retention_secs, Some(7 * 86400));
}

#[tokio::test]
async fn test_stream_create_with_retention() {
    let dir = tempfile::tempdir().unwrap();
    let state = BrokerState::with_data_dir(dir.path().to_path_buf());

    state
        .create_stream("events", Some(Duration::from_secs(24 * 3600)), None)
        .await
        .unwrap();

    let streams = state.list_streams().await;
    assert_eq!(streams[0].retention_secs, Some(24 * 3600));
}

#[tokio::test]
async fn test_stream_create_duplicate_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let state = BrokerState::with_data_dir(dir.path().to_path_buf());

    state.create_stream("main", None, None).await.unwrap();

    let result = state.create_stream("main", None, None).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("already exists"));
}

#[tokio::test]
async fn test_stream_name_validation() {
    let dir = tempfile::tempdir().unwrap();
    let state = BrokerState::with_data_dir(dir.path().to_path_buf());

    // Invalid names
    assert!(state.create_stream("", None, None).await.is_err());
    assert!(state.create_stream("my.stream", None, None).await.is_err());
    assert!(state.create_stream("my/stream", None, None).await.is_err());
    assert!(state.create_stream("my stream", None, None).await.is_err());
    assert!(state.create_stream("-invalid", None, None).await.is_err());

    let long_name = "a".repeat(65);
    assert!(state.create_stream(&long_name, None, None).await.is_err());

    // Valid names
    assert!(state.create_stream("valid-name", None, None).await.is_ok());
    assert!(state.create_stream("stream123", None, None).await.is_ok());
}

// ─── AC#3: Stream Delete ───

#[tokio::test]
async fn test_stream_delete_removes_from_list() {
    let dir = tempfile::tempdir().unwrap();
    let state = BrokerState::with_data_dir(dir.path().to_path_buf());

    state.create_stream("main", None, None).await.unwrap();
    assert_eq!(state.list_streams().await.len(), 1);

    state.delete_stream("main").await.unwrap();
    assert!(state.list_streams().await.is_empty());
}

#[tokio::test]
async fn test_stream_delete_removes_wal_data() {
    let dir = tempfile::tempdir().unwrap();
    let state = BrokerState::with_data_dir(dir.path().to_path_buf());

    state.create_stream("main", None, None).await.unwrap();

    // Write some data to the WAL
    {
        let streams = state.streams.read().await;
        let stream = streams.get("main").unwrap();
        let _receipt = stream.wal_writer.write(b"test data").await.unwrap();
    }

    let wal_path = dir.path().join("wal").join("main.db");
    assert!(wal_path.exists());

    state.delete_stream("main").await.unwrap();
    assert!(!wal_path.exists());
}

#[tokio::test]
async fn test_stream_delete_nonexistent_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let state = BrokerState::with_data_dir(dir.path().to_path_buf());

    let result = state.delete_stream("ghost").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

// ─── AC#2: WAL Persistence ───

#[tokio::test]
async fn test_wal_persists_across_restart() {
    let dir = tempfile::tempdir().unwrap();

    // Create stream and write data
    {
        let state = BrokerState::with_data_dir(dir.path().to_path_buf());
        state
            .create_stream("main", Some(Duration::from_secs(7 * 86400)), None)
            .await
            .unwrap();

        let streams = state.streams.read().await;
        let stream = streams.get("main").unwrap();
        let receipt = stream.wal_writer.write(b"message 1").await.unwrap();
        receipt.acknowledge();
        let receipt = stream.wal_writer.write(b"message 2").await.unwrap();
        receipt.acknowledge();
    }

    // "Restart" — reload from disk
    {
        let state = BrokerState::with_data_dir(dir.path().to_path_buf());
        state.load_registry().await.unwrap();

        let streams = state.list_streams().await;
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].name, "main");

        // Verify WAL data is still present
        let streams = state.streams.read().await;
        let stream = streams.get("main").unwrap();
        let count = stream.wal_writer.record_count().await.unwrap();
        assert_eq!(count, 2);
    }
}

#[tokio::test]
async fn test_wal_no_data_loss_within_retention() {
    let dir = tempfile::tempdir().unwrap();

    // Write 100 messages
    {
        let state = BrokerState::with_data_dir(dir.path().to_path_buf());
        state
            .create_stream("main", Some(Duration::from_secs(7 * 86400)), None)
            .await
            .unwrap();

        let streams = state.streams.read().await;
        let stream = streams.get("main").unwrap();
        for i in 0..100 {
            let msg = format!("message {i}");
            let receipt = stream.wal_writer.write(msg.as_bytes()).await.unwrap();
            receipt.acknowledge();
        }
    }

    // Restart and verify all 100 messages
    {
        let state = BrokerState::with_data_dir(dir.path().to_path_buf());
        state.load_registry().await.unwrap();

        let streams = state.streams.read().await;
        let stream = streams.get("main").unwrap();
        let count = stream.wal_writer.record_count().await.unwrap();
        assert_eq!(count, 100);
    }
}

// ─── AC#5: Write-Before-Route (WAL) ───

#[tokio::test]
async fn test_wal_write_returns_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let writer = WalWriter::open(dir.path(), "test-stream").unwrap();

    let receipt = writer.write(b"hello").await.unwrap();
    assert!(receipt.record_id > 0);
    assert_eq!(receipt.stream_name, "test-stream");
    receipt.acknowledge();
}

#[tokio::test]
async fn test_wal_crc32_footer() {
    let dir = tempfile::tempdir().unwrap();
    let writer = WalWriter::open(dir.path(), "test-stream").unwrap();

    let payload = b"test payload";
    let expected_crc = crc32fast::hash(payload);

    let receipt = writer.write(payload).await.unwrap();
    receipt.acknowledge();

    // Drop writer to release exclusive lock before opening a second connection
    drop(writer);

    // Verify CRC is stored correctly
    let conn = rusqlite::Connection::open(dir.path().join("wal").join("test-stream.db")).unwrap();
    let stored_crc: i64 = conn
        .query_row("SELECT crc32 FROM wal_records WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(stored_crc, expected_crc as i64);
}

// ─── AC#4: Retention ───

#[tokio::test]
async fn test_retention_time_evicts_expired() {
    let dir = tempfile::tempdir().unwrap();
    let writer = WalWriter::open(dir.path(), "test-stream").unwrap();

    // Write an "old" message
    let _receipt = writer.write(b"old message").await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Capture current time as cutoff
    let cutoff = chrono::Utc::now().timestamp_millis();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Write a "new" message
    let _receipt = writer.write(b"new message").await.unwrap();

    // Evict old messages
    let deleted = writer.delete_before(cutoff).await.unwrap();
    assert_eq!(deleted, 1);

    let remaining = writer.record_count().await.unwrap();
    assert_eq!(remaining, 1);
}

// ─── Status Integration ───

#[tokio::test]
async fn test_status_includes_streams() {
    let dir = tempfile::tempdir().unwrap();
    let state = BrokerState::with_data_dir(dir.path().to_path_buf());

    state.create_stream("main", None, None).await.unwrap();

    // Write a message
    {
        let streams = state.streams.read().await;
        let stream = streams.get("main").unwrap();
        let receipt = stream.wal_writer.write(b"test").await.unwrap();
        receipt.acknowledge();
        stream
            .message_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    // Call the status handler directly
    let payload = serde_json::json!({"command": "status"});
    let response = wh_broker::control::handlers::dispatch("status", &payload, &state)
        .await
        .unwrap();

    assert_eq!(response["v"], 1);
    assert_eq!(response["status"], "ok");

    let streams = response["data"]["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0]["name"], "main");
    assert_eq!(streams[0]["message_count"], 1);
}
