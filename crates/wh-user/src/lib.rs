//! User profile management for Wheelhouse.
//!
//! Provides `UserStore` for registering and looking up user profiles,
//! `GitBackend` for git-versioning profiles, and `UserError` for error handling.
//!
//! User profiles are stored as YAML files in `.wh/users/{user_id}.yaml`.

pub mod error;
pub mod git;
pub mod store;

pub use error::UserError;
pub use git::GitBackend;
pub use store::{generate_user_id, UserStore};
