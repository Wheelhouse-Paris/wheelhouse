//! Broker metrics and shared state (SC-10, PP-05).
//!
//! `BrokerMetrics` tracks runtime metrics.
//! `BrokerState` is shared between the routing loop and control handler.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

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

/// Shared broker state accessed by routing loop and control handler (PP-05).
///
/// Uses `tokio::sync::RwLock` as prescribed by architecture.
pub struct BrokerState {
    pub metrics: BrokerMetrics,
    /// Subscriber count -- will be populated in Story 1.3+ when stream routing is implemented.
    pub subscriber_count: RwLock<u64>,
}

impl BrokerState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            metrics: BrokerMetrics::new(),
            subscriber_count: RwLock::new(0),
        })
    }
}

impl Default for BrokerState {
    fn default() -> Self {
        Self {
            metrics: BrokerMetrics::new(),
            subscriber_count: RwLock::new(0),
        }
    }
}
