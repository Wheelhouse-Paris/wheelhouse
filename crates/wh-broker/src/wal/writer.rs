//! SQLite WAL writer implementation (ADR-002, RT-03).
//!
//! - `PRAGMA locking_mode = EXCLUSIVE` — single writer, no concurrent access
//! - `PRAGMA journal_mode = WAL` — write-ahead logging for crash safety
//! - All I/O via `tokio::task::spawn_blocking` (PP-03)
//! - CRC32 footer on every record (FM-02)
//! - Tombstone column reserved for GDPR (ADR-002)

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::Connection;
use tokio::sync::Mutex;

use super::{WalError, WalReceipt};

/// SQLite WAL writer for a single stream.
pub struct WalWriter {
    /// Path to the SQLite database file.
    db_path: PathBuf,
    /// Stream name this writer belongs to.
    stream_name: String,
    /// SQLite connection protected by async mutex for spawn_blocking safety.
    conn: Arc<Mutex<Connection>>,
}

impl WalWriter {
    /// Open (or create) a WAL database for the given stream.
    ///
    /// Creates `{data_dir}/wal/{stream_name}.db` with the required schema.
    /// Uses `PRAGMA locking_mode = EXCLUSIVE` (RT-03) and `PRAGMA journal_mode = WAL`.
    pub fn open(data_dir: &Path, stream_name: &str) -> Result<Self, WalError> {
        let wal_dir = data_dir.join("wal");
        std::fs::create_dir_all(&wal_dir)?;

        let db_path = wal_dir.join(format!("{stream_name}.db"));
        let conn = Connection::open(&db_path)?;

        // Set pragmas (RT-03)
        conn.execute_batch(
            "PRAGMA locking_mode = EXCLUSIVE;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;",
        )?;

        // Create the WAL table with tombstone column reserved for GDPR (ADR-002)
        // Note: stream name is NOT stored per-row since each stream has its own
        // dedicated SQLite file under {data_dir}/wal/{stream_name}.db.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS wal_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                payload BLOB NOT NULL,
                crc32 INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                tombstone INTEGER DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_wal_created_at ON wal_records(created_at);",
        )?;

        Ok(Self {
            db_path,
            stream_name: stream_name.to_string(),
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Write a message to the WAL (FM-01, 5W-01).
    ///
    /// Returns a `WalReceipt` that must be consumed before forwarding the message.
    /// Computes CRC32 of the payload and stores it as a footer (FM-02).
    ///
    /// All SQLite I/O happens inside `spawn_blocking` (PP-03).
    pub async fn write(&self, payload: &[u8]) -> Result<WalReceipt, WalError> {
        let conn = Arc::clone(&self.conn);
        let payload = payload.to_vec();

        let record_id = tokio::task::spawn_blocking(move || -> Result<i64, WalError> {
            let crc = crc32fast::hash(&payload);
            let now = chrono::Utc::now().timestamp_millis();
            let conn = conn.blocking_lock();
            conn.execute(
                "INSERT INTO wal_records (payload, crc32, created_at, tombstone) VALUES (?1, ?2, ?3, 0)",
                rusqlite::params![payload, crc as i64, now],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(|e| WalError::Database(format!("spawn_blocking join error: {e}")))?
        ?;

        Ok(WalReceipt {
            record_id,
            stream_name: self.stream_name.clone(),
        })
    }

    /// Get total WAL size in bytes for this stream's database file.
    ///
    /// Uses `spawn_blocking` to avoid blocking the async runtime on metadata I/O (PP-03).
    pub async fn db_size_bytes(&self) -> Result<u64, WalError> {
        let db_path = self.db_path.clone();
        tokio::task::spawn_blocking(move || {
            let metadata = std::fs::metadata(&db_path)?;
            Ok(metadata.len())
        })
        .await
        .map_err(|e| WalError::Database(format!("spawn_blocking join error: {e}")))?
    }

    /// Delete records older than the given timestamp (milliseconds since epoch).
    ///
    /// Used for time-based retention enforcement.
    pub async fn delete_before(&self, before_timestamp_ms: i64) -> Result<u64, WalError> {
        let conn = Arc::clone(&self.conn);

        let deleted = tokio::task::spawn_blocking(move || -> Result<u64, WalError> {
            let conn = conn.blocking_lock();
            let count = conn.execute(
                "DELETE FROM wal_records WHERE created_at < ?1",
                rusqlite::params![before_timestamp_ms],
            )?;
            Ok(count as u64)
        })
        .await
        .map_err(|e| WalError::Database(format!("spawn_blocking join error: {e}")))??;

        Ok(deleted)
    }

    /// Delete oldest records to bring WAL size under the given byte limit.
    ///
    /// Used for size-based retention enforcement.
    pub async fn enforce_size_limit(&self, max_bytes: u64) -> Result<u64, WalError> {
        let current_size = self.db_size_bytes().await?;
        if current_size <= max_bytes {
            return Ok(0);
        }

        let conn = Arc::clone(&self.conn);
        let deleted = tokio::task::spawn_blocking(move || -> Result<u64, WalError> {
            let conn = conn.blocking_lock();
            // Delete oldest 10% of records to create headroom
            let total: i64 = conn.query_row(
                "SELECT COUNT(*) FROM wal_records",
                [],
                |row| row.get(0),
            )?;
            let to_delete = std::cmp::max(1, total / 10);
            let count = conn.execute(
                "DELETE FROM wal_records WHERE id IN (SELECT id FROM wal_records ORDER BY id ASC LIMIT ?1)",
                rusqlite::params![to_delete],
            )?;
            Ok(count as u64)
        })
        .await
        .map_err(|e| WalError::Database(format!("spawn_blocking join error: {e}")))?
        ?;

        Ok(deleted)
    }

    /// Count total records in the WAL.
    pub async fn record_count(&self) -> Result<u64, WalError> {
        let conn = Arc::clone(&self.conn);

        let count = tokio::task::spawn_blocking(move || -> Result<u64, WalError> {
            let conn = conn.blocking_lock();
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM wal_records", [], |row| row.get(0))?;
            Ok(count as u64)
        })
        .await
        .map_err(|e| WalError::Database(format!("spawn_blocking join error: {e}")))??;

        Ok(count)
    }

    /// Delete the WAL database file for the given stream.
    ///
    /// Removes `{data_dir}/wal/{stream_name}.db` and associated WAL/SHM files.
    pub fn delete_stream(data_dir: &Path, stream_name: &str) -> Result<(), WalError> {
        let wal_dir = data_dir.join("wal");
        let db_path = wal_dir.join(format!("{stream_name}.db"));

        // Remove main DB and SQLite auxiliary files
        for suffix in &["", "-wal", "-shm"] {
            let path = if suffix.is_empty() {
                db_path.clone()
            } else {
                PathBuf::from(format!("{}{suffix}", db_path.display()))
            };
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
        }

        Ok(())
    }

    /// Get the database path.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Get the stream name.
    pub fn stream_name(&self) -> &str {
        &self.stream_name
    }

    /// Read all records created at or after the given timestamp.
    ///
    /// Used by compaction to gather records for summary generation.
    /// All SQLite I/O happens inside `spawn_blocking` (PP-03).
    pub async fn read_records_since(
        &self,
        since_timestamp_ms: i64,
    ) -> Result<Vec<super::WalRecord>, WalError> {
        let conn = Arc::clone(&self.conn);

        let records = tokio::task::spawn_blocking(move || -> Result<Vec<super::WalRecord>, WalError> {
            let conn = conn.blocking_lock();
            let mut stmt = conn.prepare(
                "SELECT id, payload, crc32, created_at FROM wal_records WHERE created_at >= ?1 ORDER BY id ASC",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![since_timestamp_ms], |row| {
                    Ok(super::WalRecord {
                        id: row.get(0)?,
                        payload: row.get(1)?,
                        crc32: row.get::<_, i64>(2)? as u32,
                        created_at: row.get(3)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
        .map_err(|e| WalError::Database(format!("spawn_blocking join error: {e}")))?
        ?;

        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_wal_write_returns_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let writer = WalWriter::open(dir.path(), "test-stream").unwrap();

        let receipt = writer.write(b"hello world").await.unwrap();
        assert_eq!(receipt.stream_name, "test-stream");
        assert!(receipt.record_id > 0);
    }

    #[tokio::test]
    async fn test_wal_crc32_stored() {
        let dir = tempfile::tempdir().unwrap();
        let writer = WalWriter::open(dir.path(), "test-stream").unwrap();

        let payload = b"test payload for crc";
        let expected_crc = crc32fast::hash(payload) as i64;

        let _ = writer.write(payload).await.unwrap();

        // Verify CRC in database
        let conn = writer.conn.lock().await;
        let stored_crc: i64 = conn
            .query_row("SELECT crc32 FROM wal_records WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(stored_crc, expected_crc);
    }

    #[tokio::test]
    async fn test_wal_record_count() {
        let dir = tempfile::tempdir().unwrap();
        let writer = WalWriter::open(dir.path(), "test-stream").unwrap();

        assert_eq!(writer.record_count().await.unwrap(), 0);

        writer.write(b"msg1").await.unwrap();
        writer.write(b"msg2").await.unwrap();
        writer.write(b"msg3").await.unwrap();

        assert_eq!(writer.record_count().await.unwrap(), 3);
    }

    #[tokio::test]
    async fn test_wal_delete_before() {
        let dir = tempfile::tempdir().unwrap();
        let writer = WalWriter::open(dir.path(), "test-stream").unwrap();

        writer.write(b"old message").await.unwrap();
        // Small delay to separate timestamps
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let cutoff = chrono::Utc::now().timestamp_millis();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        writer.write(b"new message").await.unwrap();

        let deleted = writer.delete_before(cutoff).await.unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(writer.record_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_wal_delete_stream() {
        let dir = tempfile::tempdir().unwrap();
        let writer = WalWriter::open(dir.path(), "test-stream").unwrap();
        writer.write(b"data").await.unwrap();

        let db_path = writer.db_path().to_path_buf();
        assert!(db_path.exists());

        // Drop writer to release connection
        drop(writer);

        WalWriter::delete_stream(dir.path(), "test-stream").unwrap();
        assert!(!db_path.exists());
    }

    #[tokio::test]
    async fn test_wal_tombstone_column_exists() {
        let dir = tempfile::tempdir().unwrap();
        let writer = WalWriter::open(dir.path(), "test-stream").unwrap();
        writer.write(b"data").await.unwrap();

        // Verify tombstone column exists and defaults to 0
        let conn = writer.conn.lock().await;
        let tombstone: i64 = conn
            .query_row(
                "SELECT tombstone FROM wal_records WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tombstone, 0);
    }
}
