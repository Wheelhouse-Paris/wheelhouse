#![warn(missing_docs, unused)]

//! # wh-skill
//!
//! Skill definition, manifest parsing, directory scanning, and git-based
//! skill repository for Wheelhouse.
//!
//! A skill is a set of versioned markdown files stored in git. Each skill
//! consists of a `skill.md` manifest (YAML front-matter + markdown body)
//! and ordered step files in a `steps/` subdirectory.

/// `.wh` skill configuration parsing.
pub mod config;
/// Skill directory scanning and filesystem loading.
pub mod directory;
/// Error types for skill operations.
pub mod error;
/// Skill manifest parsing and validation.
pub mod manifest;
/// Git-based skill repository operations.
pub mod repository;

pub use config::{SkillRef, SkillsConfig};
pub use directory::SkillDirectory;
pub use error::SkillError;
pub use manifest::SkillManifest;
pub use repository::SkillRepository;
