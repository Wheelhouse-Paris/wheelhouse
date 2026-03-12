//! User profile management for Wheelhouse surfaces.
//!
//! This crate provides user profile registration, lookup, and git persistence.
//! Surfaces use it to register users on first interaction (FR58) and attribute
//! stream objects with `user_id` (FR59).
//!
//! # Architecture
//!
//! - Profiles stored as YAML files in `.wh/users/{user_id}.yaml`
//! - Git-versioned for traceability and GDPR purgeability
//! - `user_id` is attribution metadata only, NOT an authentication credential
//! - Cross-cutting concern: used by CLI, Telegram, and future surfaces

pub mod error;
pub mod git;
pub mod store;

pub use error::UserError;
pub use git::GitBackend;
pub use store::{generate_user_id, UserStore};
