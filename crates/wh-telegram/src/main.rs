//! Telegram surface runner binary entry point.
//!
//! Minimal — delegates to library. Only file that may use `anyhow` (SCV-04).
//!
//! Startup sequence:
//!   1. Initialize tracing (JSON format with env-filter)
//!   2. Read config from environment (`TelegramConfig::from_env()`)
//!   3. Connect ZMQ bridge to broker
//!   4. Initialize Telegram bot, user store, chat mapping
//!   5. Start runner loop (inbound/outbound tasks + bot dispatcher)
//!
//! On startup failure: logs error, prints human-readable message, exits with code 1.
//! No crash loop, no retry — Podman handles restarts.

use std::sync::Arc;

use teloxide::prelude::*;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use wh_telegram::bridge::ZmqBridge;
use wh_telegram::config::TelegramConfig;
use wh_telegram::mapping::ChatMapping;
use wh_telegram::surface::TelegramSurface;
use wh_user::UserStore;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize structured JSON logging with env-filter
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Step 1: Read configuration from environment
    let config = match TelegramConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "configuration error");
            eprintln!("wh-telegram: {e}");
            std::process::exit(1);
        }
    };

    info!(
        stream = %config.stream_name(),
        surface = %config.surface_name(),
        wh_url = %config.wh_url(),
        "telegram surface starting"
    );

    // Step 2: Connect ZMQ bridge to broker (AC-4: exit 1 on failure)
    let publisher_id = format!("surface-{}", config.surface_name());
    let bridge =
        match ZmqBridge::connect(config.wh_url(), config.stream_name(), &publisher_id).await {
            Ok(b) => b,
            Err(e) => {
                error!(error = %e, "failed to connect to broker");
                eprintln!(
                "wh-telegram: failed to connect to broker at {} -- is `wh broker start` running?",
                config.wh_url()
            );
                std::process::exit(1);
            }
        };

    // Split bridge into separate publisher/subscriber handles for concurrent use
    // without mutex contention (H1 fix).
    let (mut publisher, mut subscriber) = bridge.split();

    // Step 3: Initialize user store and chat mapping
    let data_dir = std::env::var("WH_DATA_DIR").unwrap_or_else(|_| "./data".to_string());
    let user_store = UserStore::new(std::path::Path::new(&data_dir).join("users"));
    let chat_mapping_path = std::path::Path::new(&data_dir).join("telegram");
    let chat_mapping = ChatMapping::new(&chat_mapping_path).unwrap_or_else(|e| {
        error!(error = %e, path = %chat_mapping_path.display(), "failed to load chat mappings");
        panic!(
            "failed to create chat mapping at {}: {e}",
            chat_mapping_path.display()
        );
    });

    // Step 4: Create Telegram surface
    let surface = Arc::new(TelegramSurface::new(
        config.clone(),
        user_store,
        chat_mapping,
    ));
    let outbound_rx = surface.take_outbound_rx();

    // Step 5: Create Telegram bot
    let bot = Bot::new(config.bot_token());

    // Cancellation token for graceful shutdown (SC-06)
    let cancel = CancellationToken::new();

    // Spawn outbound publication task: drains surface outbound channel -> ZMQ publisher
    let outbound_cancel = cancel.clone();
    let outbound_handle = tokio::spawn(async move {
        if let Some(mut rx) = outbound_rx {
            loop {
                tokio::select! {
                    biased;
                    _ = outbound_cancel.cancelled() => {
                        info!("outbound task shutting down");
                        break;
                    }
                    msg = rx.recv() => {
                        match msg {
                            Some(text_msg) => {
                                if let Err(e) = publisher.publish(&text_msg).await {
                                    error!(error = %e, "failed to publish outbound message");
                                }
                            }
                            None => {
                                info!("outbound channel closed");
                                break;
                            }
                        }
                    }
                }
            }
        }
    });

    // Spawn inbound subscription task: ZMQ subscriber -> surface handle_outgoing
    let inbound_surface = surface.clone();
    let inbound_bot = bot.clone();
    let inbound_cancel = cancel.clone();
    let inbound_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = inbound_cancel.cancelled() => {
                    info!("inbound task shutting down");
                    break;
                }
                result = subscriber.recv() => {
                    match result {
                        Ok(Some((text_msg, _publisher_id))) => {
                            // Only deliver messages with reply_to_user_id set
                            if !text_msg.reply_to_user_id.is_empty() {
                                if let Err(e) = inbound_surface
                                    .handle_outgoing(&inbound_bot, &text_msg)
                                    .await
                                {
                                    error!(error = %e, "failed to deliver inbound message to Telegram");
                                }
                            }
                        }
                        Ok(None) => {
                            // Non-TextMessage or self-echo, skip
                        }
                        Err(e) => {
                            error!(error = %e, "inbound recv error");
                            // Yield on error path to prevent CPU spin (SC-09)
                            tokio::task::yield_now().await;
                        }
                    }
                }
            }
        }
    });

    // Start the teloxide bot dispatcher
    let dispatcher_surface = surface.clone();
    let dispatcher_bot = bot.clone();
    let dispatcher_cancel = cancel.clone();

    info!("telegram bot dispatcher starting");

    // Spawn SIGTERM/SIGINT handler for graceful shutdown
    let signal_cancel = cancel.clone();
    tokio::spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to register SIGTERM handler");

        tokio::select! {
            _ = ctrl_c => {
                info!("received SIGINT, shutting down");
            }
            _ = sigterm.recv() => {
                info!("received SIGTERM, shutting down");
            }
        }
        signal_cancel.cancel();
    });

    // Run teloxide dispatcher with cancellation support
    // Note: we do NOT call .enable_ctrlc_handler() because we have our own
    // signal handler above that cancels the CancellationToken (H2 fix).
    let handler = Update::filter_message().endpoint(move |bot: Bot, msg: Message| {
        let surface = dispatcher_surface.clone();
        async move {
            if let Err(e) = surface.handle_incoming(&bot, &msg).await {
                surface.send_error_to_user(&bot, msg.chat.id.0, &e).await;
            }
            respond(())
        }
    });

    let mut dispatcher = Dispatcher::builder(dispatcher_bot, handler).build();

    tokio::select! {
        biased;
        _ = dispatcher_cancel.cancelled() => {
            info!("cancellation received, stopping dispatcher");
        }
        _ = dispatcher.dispatch() => {
            info!("dispatcher exited");
        }
    }

    // Graceful shutdown: cancel all tasks and wait
    cancel.cancel();

    info!("waiting for tasks to drain");
    let _ = tokio::join!(outbound_handle, inbound_handle);

    info!("telegram surface shut down cleanly");
    Ok(())
}
