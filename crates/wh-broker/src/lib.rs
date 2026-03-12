//! Wheelhouse broker library.
//!
//! Provides the core broker functionality: socket binding, routing loop,
//! control socket, and metrics. The broker binds exclusively on `127.0.0.1`
//! as a security invariant (ADR-001, NFR-S1).

pub mod config;
pub mod control;
pub mod error;
pub mod metrics;
pub mod routing;

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
/// Initializes tracing, creates shared state, and spawns the routing and control tasks.
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
        "starting Wheelhouse — binding sockets"
    );

    let state = BrokerState::new();
    let cancel = CancellationToken::new();

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
        "control socket started — Wheelhouse ready"
    );

    // Wait for shutdown signal (SIGINT/SIGTERM)
    wait_for_shutdown().await;

    tracing::info!("shutdown signal received — stopping Wheelhouse");
    cancel.cancel();

    // Wait for tasks to complete
    let _ = routing_handle.await;
    let _ = control_handle.await;

    tracing::info!(
        elapsed_ms = startup.elapsed().as_millis() as u64,
        "Wheelhouse stopped"
    );

    Ok(())
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
