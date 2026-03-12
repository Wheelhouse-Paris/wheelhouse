//! Wheelhouse broker library.
//!
//! Provides the core broker functionality: socket binding, routing loop,
//! control socket, metrics, WAL persistence, and stream management.
//! The broker binds exclusively on `127.0.0.1` as a security invariant (ADR-001, NFR-S1).

pub mod config;
pub mod control;
pub mod deploy;
pub mod error;
pub mod metrics;
pub mod registry;
pub mod routing;
pub mod wal;

use std::sync::Arc;
use std::time::Instant;

use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

use crate::config::BrokerConfig;
use crate::error::BrokerError;
use crate::metrics::BrokerState;

/// Run the broker with the given configuration.
///
/// This is the main entry point for the broker library.
/// Initializes tracing, creates shared state, loads stream registry,
/// and spawns the routing, control, and retention tasks.
#[tracing::instrument(skip_all)]
pub async fn run_broker(config: BrokerConfig) -> Result<(), BrokerError> {
    let startup = Instant::now();

    // Initialize structured JSON logging (NFR-D5, PP-07)
    init_tracing();

    tracing::info!(
        bind_address = "127.0.0.1",
        pub_port = config.pub_port(),
        sub_port = config.sub_port(),
        control_port = config.control_port(),
        data_dir = %config.data_dir().display(),
        "starting Wheelhouse — binding sockets"
    );

    // Create data directory if it doesn't exist
    std::fs::create_dir_all(config.data_dir())
        .map_err(|e| BrokerError::RoutingError(format!("Failed to create data directory: {e}")))?;

    let state = BrokerState::with_data_dir(config.data_dir().to_path_buf());
    let cancel = CancellationToken::new();

    // Load stream registry from disk (broker restart recovery)
    if let Err(e) = state.load_registry().await {
        tracing::warn!(error = %e, "failed to load stream registry — starting fresh");
    }

    tracing::info!(
        elapsed_ms = startup.elapsed().as_millis() as u64,
        "stream registry loaded"
    );

    // Spawn routing loop task (CRG-04: each socket owned by one task)
    let routing_config = config.clone();
    let routing_state = Arc::clone(&state);
    let routing_cancel = cancel.clone();
    let routing_handle = tokio::spawn(async move {
        routing::run_routing_loop(&routing_config, routing_state, routing_cancel).await
    });

    tracing::info!(
        elapsed_ms = startup.elapsed().as_millis() as u64,
        "routing loop started"
    );

    // Spawn control socket task (CRG-04: separate socket, separate task)
    let control_config = config.clone();
    let control_state = Arc::clone(&state);
    let control_cancel = cancel.clone();
    let control_handle = tokio::spawn(async move {
        control::run_control_loop(&control_config, control_state, control_cancel).await
    });

    tracing::info!(
        elapsed_ms = startup.elapsed().as_millis() as u64,
        "control socket started"
    );

    // Spawn retention enforcement task
    let retention_state = Arc::clone(&state);
    let retention_cancel = cancel.clone();
    let retention_handle = tokio::spawn(async move {
        run_retention_loop(retention_state, retention_cancel).await;
    });

    tracing::info!(
        elapsed_ms = startup.elapsed().as_millis() as u64,
        "retention task started — Wheelhouse ready"
    );

    // Wait for shutdown signal (SIGINT/SIGTERM)
    wait_for_shutdown().await;

    tracing::info!("shutdown signal received — stopping Wheelhouse");
    cancel.cancel();

    // Wait for tasks to complete
    let _ = routing_handle.await;
    let _ = control_handle.await;
    let _ = retention_handle.await;

    tracing::info!(
        elapsed_ms = startup.elapsed().as_millis() as u64,
        "Wheelhouse stopped"
    );

    Ok(())
}

/// Periodic retention enforcement loop.
///
/// Runs every 60 seconds, checks each stream for time-based and size-based retention.
/// Uses CancellationToken for clean shutdown.
async fn run_retention_loop(state: Arc<BrokerState>, cancel: CancellationToken) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));

    loop {
        tokio::select! {
            biased;

            _ = cancel.cancelled() => {
                tracing::info!("retention task shutting down");
                break;
            }

            _ = interval.tick() => {
                enforce_retention(&state).await;
            }
        }
    }
}

/// Enforce retention policies for all streams.
async fn enforce_retention(state: &Arc<BrokerState>) {
    let streams = state.streams.read().await;

    for (name, info) in streams.iter() {
        // Time-based retention
        if let Some(duration) = info.retention_duration {
            let cutoff = chrono::Utc::now().timestamp_millis()
                - (duration.as_millis() as i64);
            match info.wal_writer.delete_before(cutoff).await {
                Ok(deleted) if deleted > 0 => {
                    tracing::info!(
                        stream = %name,
                        deleted_records = deleted,
                        "retention: evicted expired records"
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(
                        stream = %name,
                        error = %e,
                        "retention: failed to evict expired records"
                    );
                }
            }
        }

        // Size-based retention
        if let Some(max_bytes) = info.retention_size_bytes {
            match info.wal_writer.enforce_size_limit(max_bytes).await {
                Ok(deleted) if deleted > 0 => {
                    tracing::info!(
                        stream = %name,
                        deleted_records = deleted,
                        "retention: evicted records to enforce size limit"
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(
                        stream = %name,
                        error = %e,
                        "retention: failed to enforce size limit"
                    );
                }
            }
        }
    }
}

/// Initialize structured JSON logging (PP-07, NFR-D5).
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .with_target(true)
        .init();
}

/// Wait for SIGINT or SIGTERM.
async fn wait_for_shutdown() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");

        tokio::select! {
            _ = ctrl_c => {
                tracing::info!("received SIGINT");
            }
            _ = sigterm.recv() => {
                tracing::info!("received SIGTERM");
            }
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.expect("failed to listen for Ctrl+C");
        tracing::info!("received Ctrl+C");
    }
}
