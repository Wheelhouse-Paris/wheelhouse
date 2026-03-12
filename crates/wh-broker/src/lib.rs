//! Wheelhouse broker library.
//!
//! Provides the core broker functionality: socket binding, routing loop,
//! control socket, metrics, WAL persistence, and stream management.
//! The broker binds exclusively on `127.0.0.1` as a security invariant (ADR-001, NFR-S1).

pub mod config;
pub mod control;
pub mod cron;
pub mod deploy;
pub mod error;
pub mod metrics;
pub mod monitor;
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

    // Create data directory if it doesn't exist (PP-03: spawn_blocking for sync I/O)
    let data_dir_path = config.data_dir().to_path_buf();
    tokio::task::spawn_blocking(move || std::fs::create_dir_all(&data_dir_path))
        .await
        .map_err(|e| BrokerError::RoutingError(format!("spawn_blocking join error: {e}")))?
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
        "retention task started"
    );

    // Spawn compaction loop task (CM-08, SC-06)
    let compaction_state = Arc::clone(&state);
    let compaction_cancel = cancel.clone();
    let compaction_interval = config.compaction_interval_secs();
    let compaction_data_dir = config.data_dir().to_path_buf();
    let compaction_handle = tokio::spawn(async move {
        run_compaction_loop(
            compaction_state,
            compaction_cancel,
            compaction_interval,
            compaction_data_dir,
        )
        .await;
    });

    tracing::info!(
        elapsed_ms = startup.elapsed().as_millis() as u64,
        compaction_interval_secs = compaction_interval,
        "compaction task started — Wheelhouse ready"
    );

    // Wait for shutdown signal (SIGINT/SIGTERM)
    wait_for_shutdown().await;

    tracing::info!("shutdown signal received — stopping Wheelhouse");
    cancel.cancel();

    // Wait for tasks to complete
    let _ = routing_handle.await;
    let _ = control_handle.await;
    let _ = retention_handle.await;
    let _ = compaction_handle.await;

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

/// Periodic compaction loop (CM-08, SC-06).
///
/// Fires compaction for all streams at the configured interval.
/// Respects `CancellationToken` for clean shutdown.
/// Per-stream mutex prevents concurrent compaction runs (CM-08).
async fn run_compaction_loop(
    state: Arc<BrokerState>,
    cancel: CancellationToken,
    interval_secs: u64,
    workspace_root: std::path::PathBuf,
) {
    let mut interval =
        tokio::time::interval(std::time::Duration::from_secs(interval_secs));

    // Skip the first immediate tick — don't compact on startup
    interval.tick().await;

    loop {
        tokio::select! {
            biased;

            _ = cancel.cancelled() => {
                tracing::info!("compaction task shutting down");
                break;
            }

            _ = interval.tick() => {
                run_compaction_for_all_streams(&state, &workspace_root, interval_secs).await;
            }
        }
    }
}

/// Run compaction for all registered streams.
async fn run_compaction_for_all_streams(
    state: &Arc<BrokerState>,
    workspace_root: &std::path::Path,
    interval_secs: u64,
) {
    let streams = state.streams.read().await;
    let stream_names: Vec<String> = streams.keys().cloned().collect();
    drop(streams);

    for stream_name in stream_names {
        let streams = state.streams.read().await;
        let Some(info) = streams.get(&stream_name) else {
            continue;
        };

        // Try to acquire per-stream compaction mutex (CM-08)
        let Ok(_guard) = info.compaction_mutex.try_lock() else {
            tracing::debug!(
                stream = %stream_name,
                "compaction: skipping — mutex busy (CM-08)"
            );
            continue;
        };

        // Compact records from one interval ago
        let since = chrono::Utc::now().timestamp_millis() - (interval_secs as i64 * 1000);
        match wal::compaction::compact_stream(
            workspace_root,
            &stream_name,
            &info.wal_writer,
            since,
        )
        .await
        {
            Ok(summary) => {
                tracing::info!(
                    stream = %stream_name,
                    record_count = summary.record_count,
                    commit_hash = %summary.commit_hash,
                    "compaction: daily summary committed"
                );
            }
            Err(e) => {
                tracing::warn!(
                    stream = %stream_name,
                    error = %e,
                    "compaction: failed"
                );
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
