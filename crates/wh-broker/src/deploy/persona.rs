//! Persona file loading for agents at startup (FR61).
//!
//! Loads SOUL.md, IDENTITY.md, and MEMORY.md from a persona directory
//! within the git workspace. SOUL.md and IDENTITY.md are required;
//! MEMORY.md is initialized as empty if missing.

use std::path::{Path, PathBuf};

use crate::deploy::DeployError;

/// The three persona files that define an agent's identity.
#[derive(Debug, Clone, PartialEq)]
pub struct PersonaFiles {
    /// Content of SOUL.md — immutable identity, purpose, values.
    pub soul: Option<String>,
    /// Content of IDENTITY.md — context, background, knowledge.
    pub identity: Option<String>,
    /// Content of MEMORY.md — mutable working memory.
    pub memory: Option<String>,
}

/// Validate that a persona path is safe (no path traversal).
///
/// Rejects paths containing `..` components or absolute paths.
fn validate_persona_path(persona_path: &str) -> Result<(), DeployError> {
    if persona_path.is_empty() {
        return Err(DeployError::PersonaLoadFailed(
            "persona path must not be empty".to_string(),
        ));
    }

    // Reject path traversal
    for component in persona_path.split('/') {
        if component == ".." {
            return Err(DeployError::PersonaLoadFailed(format!(
                "persona path '{}' contains path traversal component '..'",
                persona_path
            )));
        }
    }

    // Reject absolute paths
    if persona_path.starts_with('/') {
        return Err(DeployError::PersonaLoadFailed(format!(
            "persona path '{}' must be relative, not absolute",
            persona_path
        )));
    }

    Ok(())
}

/// Resolve the full path to the persona directory.
fn persona_dir(workspace_root: &Path, persona_path: &str) -> PathBuf {
    workspace_root.join(persona_path)
}

/// Ensure the persona directory exists, creating it if necessary.
pub fn ensure_persona_dir(workspace_root: &Path, persona_path: &str) -> Result<(), DeployError> {
    validate_persona_path(persona_path)?;
    let dir = persona_dir(workspace_root, persona_path);
    std::fs::create_dir_all(&dir).map_err(|e| {
        DeployError::PersonaLoadFailed(format!(
            "failed to create persona directory '{}': {}",
            dir.display(),
            e
        ))
    })
}

/// Initialize a missing MEMORY.md as an empty file and log a warning.
///
/// Does NOT commit to git — persona loading is a read operation.
/// The memory module handles commits separately during operation.
pub fn initialize_missing_memory(
    workspace_root: &Path,
    persona_path: &str,
) -> Result<(), DeployError> {
    let memory_path = persona_dir(workspace_root, persona_path).join("MEMORY.md");
    tracing::warn!(
        path = %memory_path.display(),
        "MEMORY.md not found in persona directory — initializing as empty file"
    );
    std::fs::write(&memory_path, "").map_err(|e| {
        DeployError::PersonaLoadFailed(format!(
            "failed to initialize MEMORY.md at '{}': {}",
            memory_path.display(),
            e
        ))
    })
}

/// Load persona files from a persona directory.
///
/// Reads SOUL.md, IDENTITY.md, and MEMORY.md from
/// `{workspace_root}/{persona_path}/`.
///
/// - SOUL.md: **required** — returns error if missing
/// - IDENTITY.md: **required** — returns error if missing
/// - MEMORY.md: **optional** — initialized as empty file if missing (FR61),
///   with a warning log entry
pub fn load_persona(
    workspace_root: &Path,
    persona_path: &str,
) -> Result<PersonaFiles, DeployError> {
    validate_persona_path(persona_path)?;

    let dir = persona_dir(workspace_root, persona_path);

    // Verify persona directory exists
    if !dir.exists() {
        return Err(DeployError::PersonaLoadFailed(format!(
            "persona directory '{}' does not exist",
            dir.display()
        )));
    }

    // Load SOUL.md (required)
    let soul_path = dir.join("SOUL.md");
    if !soul_path.exists() {
        return Err(DeployError::PersonaLoadFailed(format!(
            "required persona file SOUL.md not found at '{}'",
            soul_path.display()
        )));
    }
    let soul = std::fs::read_to_string(&soul_path).map_err(|e| {
        DeployError::PersonaLoadFailed(format!(
            "failed to read SOUL.md at '{}': {}",
            soul_path.display(),
            e
        ))
    })?;

    // Load IDENTITY.md (required)
    let identity_path = dir.join("IDENTITY.md");
    if !identity_path.exists() {
        return Err(DeployError::PersonaLoadFailed(format!(
            "required persona file IDENTITY.md not found at '{}'",
            identity_path.display()
        )));
    }
    let identity = std::fs::read_to_string(&identity_path).map_err(|e| {
        DeployError::PersonaLoadFailed(format!(
            "failed to read IDENTITY.md at '{}': {}",
            identity_path.display(),
            e
        ))
    })?;

    // Load MEMORY.md (optional — initialize as empty if missing)
    let memory_path = dir.join("MEMORY.md");
    let memory = if memory_path.exists() {
        std::fs::read_to_string(&memory_path).map_err(|e| {
            DeployError::PersonaLoadFailed(format!(
                "failed to read MEMORY.md at '{}': {}",
                memory_path.display(),
                e
            ))
        })?
    } else {
        // FR61: Initialize MEMORY.md as empty rather than failing
        initialize_missing_memory(workspace_root, persona_path)?;
        String::new()
    };

    Ok(PersonaFiles {
        soul: Some(soul),
        identity: Some(identity),
        memory: Some(memory),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_persona_all_files_present() {
        let dir = tempfile::tempdir().unwrap();
        let persona_dir = dir.path().join("agents/donna");
        std::fs::create_dir_all(&persona_dir).unwrap();
        std::fs::write(persona_dir.join("SOUL.md"), "I am Donna.").unwrap();
        std::fs::write(persona_dir.join("IDENTITY.md"), "Chief of staff.").unwrap();
        std::fs::write(persona_dir.join("MEMORY.md"), "Last action: scaled.").unwrap();

        let persona = load_persona(dir.path(), "agents/donna").unwrap();
        assert_eq!(persona.soul.as_deref(), Some("I am Donna."));
        assert_eq!(persona.identity.as_deref(), Some("Chief of staff."));
        assert_eq!(persona.memory.as_deref(), Some("Last action: scaled."));
    }

    #[test]
    fn load_persona_missing_memory_initializes_empty() {
        let dir = tempfile::tempdir().unwrap();
        let persona_dir = dir.path().join("agents/donna");
        std::fs::create_dir_all(&persona_dir).unwrap();
        std::fs::write(persona_dir.join("SOUL.md"), "I am Donna.").unwrap();
        std::fs::write(persona_dir.join("IDENTITY.md"), "Chief of staff.").unwrap();

        let persona = load_persona(dir.path(), "agents/donna").unwrap();
        assert_eq!(persona.memory, Some(String::new()));
        // File should now exist
        assert!(persona_dir.join("MEMORY.md").exists());
    }

    #[test]
    fn load_persona_missing_soul_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let persona_dir = dir.path().join("agents/donna");
        std::fs::create_dir_all(&persona_dir).unwrap();
        std::fs::write(persona_dir.join("IDENTITY.md"), "Chief of staff.").unwrap();
        std::fs::write(persona_dir.join("MEMORY.md"), "Some memory.").unwrap();

        let result = load_persona(dir.path(), "agents/donna");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, DeployError::PersonaLoadFailed(_)));
        assert!(err.to_string().contains("SOUL.md"));
    }

    #[test]
    fn load_persona_missing_identity_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let persona_dir = dir.path().join("agents/donna");
        std::fs::create_dir_all(&persona_dir).unwrap();
        std::fs::write(persona_dir.join("SOUL.md"), "I am Donna.").unwrap();
        std::fs::write(persona_dir.join("MEMORY.md"), "Some memory.").unwrap();

        let result = load_persona(dir.path(), "agents/donna");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, DeployError::PersonaLoadFailed(_)));
        assert!(err.to_string().contains("IDENTITY.md"));
    }

    #[test]
    fn load_persona_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let result = load_persona(dir.path(), "../evil/");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("path traversal"));
    }

    #[test]
    fn load_persona_rejects_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let result = load_persona(dir.path(), "/etc/passwd");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("relative"));
    }

    #[test]
    fn load_persona_rejects_empty_path() {
        let dir = tempfile::tempdir().unwrap();
        let result = load_persona(dir.path(), "");
        assert!(result.is_err());
    }

    #[test]
    fn ensure_persona_dir_creates_directory() {
        let dir = tempfile::tempdir().unwrap();
        let persona_path = "agents/newagent";
        ensure_persona_dir(dir.path(), persona_path).unwrap();
        assert!(dir.path().join(persona_path).exists());
    }

    #[test]
    fn validate_persona_path_rejects_double_dot() {
        assert!(validate_persona_path("agents/../evil").is_err());
        assert!(validate_persona_path("..").is_err());
        assert!(validate_persona_path("foo/../../bar").is_err());
    }

    #[test]
    fn validate_persona_path_accepts_valid_paths() {
        assert!(validate_persona_path("agents/donna").is_ok());
        assert!(validate_persona_path("agents/donna/").is_ok());
        assert!(validate_persona_path("personas/my-agent-1").is_ok());
    }

    #[test]
    fn load_persona_nonexistent_directory_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let result = load_persona(dir.path(), "agents/nonexistent");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }
}
