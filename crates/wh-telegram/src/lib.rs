//! Telegram surface for Wheelhouse.
//!
//! Connects Telegram users to Wheelhouse agents via streams.
//! Users interact with agents by sending Telegram messages; agent responses
//! are delivered back to the correct Telegram chat.
//!
//! Key components:
//! - `TelegramConfig`: reads bot token and stream name from environment
//! - `TelegramSurface`: core surface connecting Telegram to streams
//! - `ChatMapping`: bidirectional user_id <-> chat_id mapping with YAML persistence
//! - `AckTracker`: 5-second "Working on it..." timeout for slow responses
//! - `TelegramError` + `sanitize_for_user`: error handling with RT-B1 compliance

pub mod ack;
pub mod config;
pub mod error;
pub mod mapping;
pub mod surface;

pub use ack::AckTracker;
pub use config::TelegramConfig;
pub use error::{sanitize_for_user, TelegramError};
pub use mapping::ChatMapping;
pub use surface::TelegramSurface;
