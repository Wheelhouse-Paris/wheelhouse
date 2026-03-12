#![warn(missing_docs, unused)]

//! # wh-skill
//!
//! Skill definition, manifest parsing, directory scanning, git-based
//! skill repository, and skill invocation pipeline for Wheelhouse.
//!
//! A skill is a set of versioned markdown files stored in git. Each skill
//! consists of a `skill.md` manifest (YAML front-matter + markdown body)
//! and ordered step files in a `steps/` subdirectory.
//!
//! The invocation pipeline validates skill allowlists (FM-05), loads
//! skills from git repositories, and executes them with progress
//! reporting (CM-06).

/// Skill allowlist validation (FM-05).
pub mod allowlist;
/// `.wh` skill configuration parsing.
pub mod config;
/// Skill directory scanning and filesystem loading.
pub mod directory;
/// Error types for skill operations.
pub mod error;
/// Skill executor trait and local implementation.
pub mod executor;
/// Skill invocation domain types, proto conversions, and builders.
pub mod invocation;
/// Skill manifest parsing and validation.
pub mod manifest;
/// Skill invocation pipeline: allowlist + load + execute.
pub mod pipeline;
/// Git-based skill repository operations.
pub mod repository;

pub use allowlist::SkillAllowlist;
pub use config::{SkillRef, SkillsConfig};
pub use directory::SkillDirectory;
pub use error::SkillError;
pub use executor::{LocalSkillExecutor, SkillExecutorEvent};
pub use invocation::{SkillInvocationOutcome, SkillInvocationRequest};
pub use manifest::SkillManifest;
pub use pipeline::InvocationPipeline;
pub use repository::SkillRepository;
