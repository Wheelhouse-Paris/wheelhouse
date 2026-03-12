//! Broker metrics, shared state, and stream registry (SC-10, PP-05).
//!
//! `BrokerMetrics` tracks runtime metrics.
//! `BrokerState` is shared between the routing loop and control handler.
//! Stream registry manages named streams with WAL persistence.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::RwLock;

use crate::wal::{WalError, WalWriter};

/// Runtime metrics for the broker (SC-10).
pub struct BrokerMetrics {
    /// When the broker started.
    start_time: Instant,
    /// Count of panics caught in the routing loop.
    pub panic_count: AtomicU64,
}

impl BrokerMetrics {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            panic_count: AtomicU64::new(0),
        }
    }

    /// Broker uptime in seconds.
    pub fn uptime_secs(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }

    /// Current panic count.
    pub fn get_panic_count(&self) -> u64 {
        self.panic_count.load(Ordering::Relaxed)
    }
}

impl Default for BrokerMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Information about a registered stream.
pub struct StreamInfo {
    /// Stream name.
    pub name: String,
    /// Time-based retention duration (e.g., 7 days).
    pub retention_duration: Option<Duration>,
    /// Size-based retention limit in bytes.
    pub retention_size_bytes: Option<u64>,
    /// When the stream was created.
    pub created_at: SystemTime,
    /// Total messages written to WAL.
    pub message_count: AtomicU64,
    /// Per-stream monotonically increasing sequence number (FR54).
    /// Initialized from WAL record count on startup for continuity across restarts.
    pub sequence_counter: AtomicU64,
    /// WAL writer for this stream.
    pub wal_writer: WalWriter,
}

/// Serializable stream metadata for persistence.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct StreamMetadata {
    pub name: String,
    pub retention_secs: Option<u64>,
    pub retention_size_bytes: Option<u64>,
    pub created_at_epoch_ms: i64,
}

/// Stream name validation error.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("Stream '{0}' already exists")]
    AlreadyExists(String),

    #[error("Stream '{0}' not found")]
    NotFound(String),

    #[error("Invalid stream name: {0}")]
    InvalidName(String),

    #[error("WAL error: {0}")]
    Wal(#[from] WalError),

    #[error("Registry I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Validate a stream name.
///
/// Stream names must be 1-64 characters, alphanumeric + hyphens only.
pub fn validate_stream_name(name: &str) -> Result<(), StreamError> {
    if name.is_empty() || name.len() > 64 {
        return Err(StreamError::InvalidName(
            "Stream name must be 1-64 characters".to_string(),
        ));
    }

    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(StreamError::InvalidName(
            "Stream name must contain only alphanumeric characters and hyphens".to_string(),
        ));
    }

    if name.starts_with('-') || name.ends_with('-') {
        return Err(StreamError::InvalidName(
            "Stream name must not start or end with a hyphen".to_string(),
        ));
    }

    Ok(())
}

/// Parse a retention duration string like "7d", "24h", "30m".
pub fn parse_retention_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim().to_lowercase();

    if let Some(days) = s.strip_suffix('d') {
        let n: u64 = days.parse().map_err(|_| format!("Invalid duration: {s}"))?;
        Ok(Duration::from_secs(n * 86400))
    } else if let Some(hours) = s.strip_suffix('h') {
        let n: u64 = hours.parse().map_err(|_| format!("Invalid duration: {s}"))?;
        Ok(Duration::from_secs(n * 3600))
    } else if let Some(mins) = s.strip_suffix('m') {
        let n: u64 = mins.parse().map_err(|_| format!("Invalid duration: {s}"))?;
        Ok(Duration::from_secs(n * 60))
    } else {
        Err(format!("Invalid retention format '{s}': use e.g. '7d', '24h', '30m'"))
    }
}

/// Parse a retention size string like "500mb", "1gb".
pub fn parse_retention_size(s: &str) -> Result<u64, String> {
    let s = s.trim().to_lowercase();

    if let Some(gb) = s.strip_suffix("gb") {
        let n: u64 = gb.parse().map_err(|_| format!("Invalid size: {s}"))?;
        Ok(n * 1024 * 1024 * 1024)
    } else if let Some(mb) = s.strip_suffix("mb") {
        let n: u64 = mb.parse().map_err(|_| format!("Invalid size: {s}"))?;
        Ok(n * 1024 * 1024)
    } else {
        Err(format!("Invalid retention size format '{s}': use e.g. '500mb', '1gb'"))
    }
}

/// Format a Duration as a human-readable retention string.
pub fn format_retention_duration(d: &Duration) -> String {
    let secs = d.as_secs();
    if secs % 86400 == 0 {
        format!("{}d", secs / 86400)
    } else if secs % 3600 == 0 {
        format!("{}h", secs / 3600)
    } else if secs % 60 == 0 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// Shared broker state accessed by routing loop and control handler (PP-05).
///
/// Uses `tokio::sync::RwLock` as prescribed by architecture.
pub struct BrokerState {
    pub metrics: BrokerMetrics,
    /// Subscriber count — populated when stream routing is implemented.
    pub subscriber_count: RwLock<u64>,
    /// Stream registry — maps stream name to StreamInfo.
    pub streams: RwLock<HashMap<String, StreamInfo>>,
    /// Data directory for WAL files and stream registry.
    pub data_dir: PathBuf,
}

impl BrokerState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            metrics: BrokerMetrics::new(),
            subscriber_count: RwLock::new(0),
            streams: RwLock::new(HashMap::new()),
            data_dir: PathBuf::from("."),
        })
    }

    /// Create a new BrokerState with the specified data directory.
    pub fn with_data_dir(data_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            metrics: BrokerMetrics::new(),
            subscriber_count: RwLock::new(0),
            streams: RwLock::new(HashMap::new()),
            data_dir,
        })
    }

    /// Create a named stream with optional retention settings.
    pub async fn create_stream(
        self: &Arc<Self>,
        name: &str,
        retention_duration: Option<Duration>,
        retention_size_bytes: Option<u64>,
    ) -> Result<(), StreamError> {
        validate_stream_name(name)?;

        let mut streams = self.streams.write().await;
        if streams.contains_key(name) {
            return Err(StreamError::AlreadyExists(name.to_string()));
        }

        let wal_writer = WalWriter::open(&self.data_dir, name)?;

        let record_count = wal_writer.record_count().await.unwrap_or(0);

        let info = StreamInfo {
            name: name.to_string(),
            retention_duration,
            retention_size_bytes,
            created_at: SystemTime::now(),
            message_count: AtomicU64::new(0),
            sequence_counter: AtomicU64::new(record_count),
            wal_writer,
        };

        streams.insert(name.to_string(), info);
        drop(streams);

        self.persist_registry().await?;

        tracing::info!(stream = name, "stream created");
        Ok(())
    }

    /// Delete a named stream and its WAL data.
    ///
    /// Removes the stream from the registry first (dropping the WalWriter and
    /// closing the SQLite connection), then deletes the WAL files from disk.
    /// If WAL file cleanup fails, the stream is still removed from the registry
    /// and a warning is logged — the orphaned files are harmless.
    pub async fn delete_stream(self: &Arc<Self>, name: &str) -> Result<(), StreamError> {
        let mut streams = self.streams.write().await;
        if streams.remove(name).is_none() {
            return Err(StreamError::NotFound(name.to_string()));
        }
        drop(streams); // Drop StreamInfo → closes WalWriter's SQLite connection

        // Delete WAL files after connection is closed
        if let Err(e) = WalWriter::delete_stream(&self.data_dir, name) {
            tracing::warn!(
                stream = name,
                error = %e,
                "failed to delete WAL files — orphaned files may remain"
            );
        }

        self.persist_registry().await?;

        tracing::info!(stream = name, "stream deleted");
        Ok(())
    }

    /// List all streams with their metadata.
    pub async fn list_streams(&self) -> Vec<StreamMetadata> {
        let streams = self.streams.read().await;
        streams
            .values()
            .map(|info| StreamMetadata {
                name: info.name.clone(),
                retention_secs: info.retention_duration.map(|d| d.as_secs()),
                retention_size_bytes: info.retention_size_bytes,
                created_at_epoch_ms: info
                    .created_at
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64,
            })
            .collect()
    }

    /// Persist stream registry to `{data_dir}/streams.json`.
    async fn persist_registry(&self) -> Result<(), StreamError> {
        let streams = self.streams.read().await;
        let metadata: Vec<StreamMetadata> = streams
            .values()
            .map(|info| StreamMetadata {
                name: info.name.clone(),
                retention_secs: info.retention_duration.map(|d| d.as_secs()),
                retention_size_bytes: info.retention_size_bytes,
                created_at_epoch_ms: info
                    .created_at
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64,
            })
            .collect();
        drop(streams);

        let json = serde_json::to_string_pretty(&metadata)
            .map_err(|e| StreamError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

        let data_dir = self.data_dir.clone();
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            std::fs::create_dir_all(&data_dir)?;
            std::fs::write(data_dir.join("streams.json"), json)?;
            Ok(())
        })
        .await
        .map_err(|e| StreamError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))??;

        Ok(())
    }

    /// Load stream registry from `{data_dir}/streams.json` on startup.
    pub async fn load_registry(self: &Arc<Self>) -> Result<(), StreamError> {
        let registry_path = self.data_dir.join("streams.json");

        let path_clone = registry_path.clone();
        let json_opt = tokio::task::spawn_blocking(move || -> std::io::Result<Option<String>> {
            if !path_clone.exists() {
                return Ok(None);
            }
            Ok(Some(std::fs::read_to_string(&path_clone)?))
        })
        .await
        .map_err(|e| StreamError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))??;

        let Some(json) = json_opt else {
            return Ok(());
        };
        let metadata: Vec<StreamMetadata> = serde_json::from_str(&json)
            .map_err(|e| StreamError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

        let mut streams = self.streams.write().await;
        for meta in metadata {
            let wal_writer = WalWriter::open(&self.data_dir, &meta.name)?;
            let message_count = wal_writer.record_count().await.unwrap_or(0);

            let created_at = SystemTime::UNIX_EPOCH
                + Duration::from_millis(meta.created_at_epoch_ms as u64);

            let info = StreamInfo {
                name: meta.name.clone(),
                retention_duration: meta.retention_secs.map(Duration::from_secs),
                retention_size_bytes: meta.retention_size_bytes,
                created_at,
                message_count: AtomicU64::new(message_count),
                sequence_counter: AtomicU64::new(message_count),
                wal_writer,
            };

            streams.insert(meta.name, info);
        }

        tracing::info!(
            stream_count = streams.len(),
            "stream registry loaded from disk"
        );

        Ok(())
    }
}

impl Default for BrokerState {
    fn default() -> Self {
        Self {
            metrics: BrokerMetrics::new(),
            subscriber_count: RwLock::new(0),
            streams: RwLock::new(HashMap::new()),
            data_dir: PathBuf::from("."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_stream_name_valid() {
        assert!(validate_stream_name("main").is_ok());
        assert!(validate_stream_name("my-stream").is_ok());
        assert!(validate_stream_name("stream123").is_ok());
        assert!(validate_stream_name("a").is_ok());
    }

    #[test]
    fn test_validate_stream_name_invalid() {
        assert!(validate_stream_name("").is_err());
        assert!(validate_stream_name("a".repeat(65).as_str()).is_err());
        assert!(validate_stream_name("my.stream").is_err());
        assert!(validate_stream_name("my/stream").is_err());
        assert!(validate_stream_name("my stream").is_err());
        assert!(validate_stream_name("-invalid").is_err());
        assert!(validate_stream_name("invalid-").is_err());
    }

    #[test]
    fn test_parse_retention_duration() {
        assert_eq!(parse_retention_duration("7d").unwrap(), Duration::from_secs(7 * 86400));
        assert_eq!(parse_retention_duration("24h").unwrap(), Duration::from_secs(24 * 3600));
        assert_eq!(parse_retention_duration("30m").unwrap(), Duration::from_secs(30 * 60));
        assert!(parse_retention_duration("invalid").is_err());
        assert!(parse_retention_duration("7x").is_err());
    }

    #[test]
    fn test_parse_retention_size() {
        assert_eq!(parse_retention_size("500mb").unwrap(), 500 * 1024 * 1024);
        assert_eq!(parse_retention_size("1gb").unwrap(), 1024 * 1024 * 1024);
        assert!(parse_retention_size("invalid").is_err());
    }

    #[test]
    fn test_format_retention_duration() {
        assert_eq!(format_retention_duration(&Duration::from_secs(7 * 86400)), "7d");
        assert_eq!(format_retention_duration(&Duration::from_secs(24 * 3600)), "1d"); // 24h = 1d
        assert_eq!(format_retention_duration(&Duration::from_secs(12 * 3600)), "12h");
        assert_eq!(format_retention_duration(&Duration::from_secs(30 * 60)), "30m");
    }

    #[tokio::test]
    async fn test_stream_create_list_delete() {
        let dir = tempfile::tempdir().unwrap();
        let state = BrokerState::with_data_dir(dir.path().to_path_buf());

        // Create stream
        state
            .create_stream("main", Some(Duration::from_secs(7 * 86400)), None)
            .await
            .unwrap();

        // List
        let streams = state.list_streams().await;
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].name, "main");

        // Duplicate
        let result = state
            .create_stream("main", None, None)
            .await;
        assert!(matches!(result, Err(StreamError::AlreadyExists(_))));

        // Delete
        state.delete_stream("main").await.unwrap();
        let streams = state.list_streams().await;
        assert!(streams.is_empty());

        // Delete nonexistent
        let result = state.delete_stream("ghost").await;
        assert!(matches!(result, Err(StreamError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_stream_registry_persistence() {
        let dir = tempfile::tempdir().unwrap();

        // Create streams
        {
            let state = BrokerState::with_data_dir(dir.path().to_path_buf());
            state
                .create_stream("stream-a", Some(Duration::from_secs(86400)), None)
                .await
                .unwrap();
            state
                .create_stream("stream-b", None, Some(500 * 1024 * 1024))
                .await
                .unwrap();
        }

        // Reload from disk
        {
            let state = BrokerState::with_data_dir(dir.path().to_path_buf());
            state.load_registry().await.unwrap();
            let streams = state.list_streams().await;
            assert_eq!(streams.len(), 2);
            let names: Vec<&str> = streams.iter().map(|s| s.name.as_str()).collect();
            assert!(names.contains(&"stream-a"));
            assert!(names.contains(&"stream-b"));
        }
    }
}
