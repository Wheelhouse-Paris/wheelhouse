//! WAL (Write-Ahead Log) module for stream persistence (ADR-002).
//!
//! SQLite-based WAL, append-only, broker-owned, stored locally.
//! Each stream gets its own SQLite file under `{data_dir}/wal/`.
//!
//! Key invariants:
//! - `WalReceipt` is `#[must_use]` — enforces write-before-forward at compile time (5W-01)
//! - All SQLite I/O via `tokio::task::spawn_blocking` (PP-03)
//! - `PRAGMA locking_mode = EXCLUSIVE` prevents concurrent access (RT-03)
//! - CRC32 footer on every record for crash recovery (FM-02)
//! - Tombstone column reserved for GDPR erasure (ADR-002)

pub mod writer;

pub use writer::WalWriter;

/// A receipt proving a WAL write completed successfully (5W-01).
///
/// This token type enforces write-before-forward at compile time.
/// `wal.write()` returns it; the routing loop must consume it before forwarding.
#[must_use = "WAL receipt must be consumed by forward() — write-before-forward invariant (5W-01)"]
#[derive(Debug)]
pub struct WalReceipt {
    /// The WAL record ID assigned by SQLite.
    pub record_id: i64,
    /// The stream name this receipt belongs to.
    pub stream_name: String,
}

impl WalReceipt {
    /// Consume the receipt, acknowledging the WAL write.
    pub fn acknowledge(self) {
        // Consuming `self` is sufficient — the type system enforces usage.
    }
}

/// Error type for WAL operations.
#[derive(Debug, thiserror::Error)]
pub enum WalError {
    #[error("WAL database error: {0}")]
    Database(String),

    #[error("WAL I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("CRC32 mismatch: expected {expected}, got {actual}")]
    CrcMismatch { expected: u32, actual: u32 },

    #[error("WAL capacity exceeded for stream '{stream_name}': {current_bytes} >= {max_bytes}")]
    CapacityExceeded {
        stream_name: String,
        current_bytes: u64,
        max_bytes: u64,
    },
}

impl From<rusqlite::Error> for WalError {
    fn from(e: rusqlite::Error) -> Self {
        WalError::Database(e.to_string())
    }
}
