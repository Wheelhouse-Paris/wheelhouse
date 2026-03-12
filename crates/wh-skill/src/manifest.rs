use serde::{Deserialize, Serialize};

use crate::error::SkillError;

/// A typed parameter for skill input or output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillParam {
    /// Parameter name.
    pub name: String,
    /// Parameter type (e.g., "string", "number", "boolean").
    #[serde(rename = "type")]
    pub param_type: String,
    /// Whether the parameter is required (only meaningful for inputs).
    #[serde(default)]
    pub required: bool,
}

/// The YAML front-matter of a `skill.md` manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillManifestFrontMatter {
    /// Skill name (required). Must be non-empty.
    pub name: String,
    /// Skill version (required). Must be a valid semver string.
    pub version: String,
    /// Short description of the skill (optional in YAML, stored separately from body).
    #[serde(default)]
    pub description: Option<String>,
    /// Input parameters (optional).
    #[serde(default)]
    pub inputs: Vec<SkillParam>,
    /// Output parameters (optional).
    #[serde(default)]
    pub outputs: Vec<SkillParam>,
    /// Ordered list of step file paths relative to the skill directory (required, non-empty).
    pub steps: Vec<String>,
}

/// A fully parsed skill manifest including YAML front-matter and markdown body.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillManifest {
    /// Parsed front-matter fields.
    pub front_matter: SkillManifestFrontMatter,
    /// The markdown body content (everything after the closing `---`).
    pub body: String,
}

/// Validates that a version string is a valid semver (major.minor.patch with optional pre-release).
///
/// Accepts: `1.0.0`, `1.0.0-beta.1`, `1.0.0-rc.2+build.123`
/// Rejects: `1.0`, `not-a-version`, empty strings
fn is_valid_semver(version: &str) -> bool {
    // Split off pre-release/build metadata to validate core version
    let core = version
        .split('+')
        .next()
        .unwrap_or("")
        .split('-')
        .next()
        .unwrap_or("");
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3 {
        return false;
    }
    parts.iter().all(|p| !p.is_empty() && p.parse::<u64>().is_ok())
}

impl SkillManifest {
    /// Parse a `skill.md` file content into a `SkillManifest`.
    ///
    /// The file must have YAML front-matter delimited by `---` lines,
    /// followed by an optional markdown body.
    pub fn parse(content: &str) -> Result<Self, SkillError> {
        let trimmed = content.trim();

        // Must start with ---
        if !trimmed.starts_with("---") {
            return Err(SkillError::InvalidManifest {
                reason: "skill.md must start with YAML front-matter delimited by ---".into(),
            });
        }

        // Find the closing ---
        let after_first = &trimmed[3..];
        let closing_pos = after_first.find("\n---").ok_or_else(|| SkillError::InvalidManifest {
            reason: "missing closing --- delimiter for YAML front-matter".into(),
        })?;

        let yaml_str = &after_first[..closing_pos];
        let body_start = closing_pos + 4; // skip "\n---"
        let body = if body_start < after_first.len() {
            after_first[body_start..].trim().to_string()
        } else {
            String::new()
        };

        let front_matter: SkillManifestFrontMatter =
            serde_yaml::from_str(yaml_str).map_err(|e| SkillError::InvalidManifest {
                reason: format!("YAML parse error: {e}"),
            })?;

        let manifest = SkillManifest { front_matter, body };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validate the manifest fields.
    fn validate(&self) -> Result<(), SkillError> {
        let fm = &self.front_matter;

        if fm.name.trim().is_empty() {
            return Err(SkillError::InvalidManifest {
                reason: "name must not be empty".into(),
            });
        }

        if !is_valid_semver(&fm.version) {
            return Err(SkillError::InvalidManifest {
                reason: format!(
                    "version '{}' is not valid semver (expected major.minor.patch)",
                    fm.version
                ),
            });
        }

        if fm.steps.is_empty() {
            return Err(SkillError::InvalidManifest {
                reason: "steps list must not be empty".into(),
            });
        }

        Ok(())
    }

    /// Convenience accessor for the skill name.
    pub fn name(&self) -> &str {
        &self.front_matter.name
    }

    /// Convenience accessor for the skill version.
    pub fn version(&self) -> &str {
        &self.front_matter.version
    }

    /// Convenience accessor for step file references.
    pub fn steps(&self) -> &[String] {
        &self.front_matter.steps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_manifest() {
        let content = r#"---
name: summarize
version: "1.0.0"
description: Summarize text content
inputs:
  - name: text
    type: string
    required: true
outputs:
  - name: summary
    type: string
steps:
  - steps/01-gather-context.md
  - steps/02-execute.md
---

# Summarize Skill

This skill takes text input and produces a concise summary.
"#;
        let manifest = SkillManifest::parse(content).unwrap();
        assert_eq!(manifest.name(), "summarize");
        assert_eq!(manifest.version(), "1.0.0");
        assert_eq!(manifest.front_matter.description.as_deref(), Some("Summarize text content"));
        assert_eq!(manifest.front_matter.inputs.len(), 1);
        assert_eq!(manifest.front_matter.inputs[0].name, "text");
        assert!(manifest.front_matter.inputs[0].required);
        assert_eq!(manifest.front_matter.outputs.len(), 1);
        assert_eq!(manifest.steps().len(), 2);
        assert!(manifest.body.contains("Summarize Skill"));
    }

    #[test]
    fn test_parse_minimal_manifest() {
        let content = "---\nname: test\nversion: \"0.1.0\"\nsteps:\n  - steps/01-do.md\n---\n";
        let manifest = SkillManifest::parse(content).unwrap();
        assert_eq!(manifest.name(), "test");
        assert_eq!(manifest.version(), "0.1.0");
        assert!(manifest.front_matter.inputs.is_empty());
        assert!(manifest.front_matter.outputs.is_empty());
        assert!(manifest.body.is_empty());
    }

    #[test]
    fn test_reject_missing_name() {
        let content = "---\nversion: \"1.0.0\"\nsteps:\n  - steps/01-do.md\n---\n";
        let err = SkillManifest::parse(content).unwrap_err();
        assert!(matches!(err, SkillError::InvalidManifest { .. }));
    }

    #[test]
    fn test_reject_empty_name() {
        let content = "---\nname: \"\"\nversion: \"1.0.0\"\nsteps:\n  - steps/01-do.md\n---\n";
        let err = SkillManifest::parse(content).unwrap_err();
        match &err {
            SkillError::InvalidManifest { reason } => assert!(reason.contains("name")),
            _ => panic!("expected InvalidManifest, got {err:?}"),
        }
    }

    #[test]
    fn test_reject_invalid_semver() {
        let content = "---\nname: test\nversion: \"not-a-version\"\nsteps:\n  - steps/01-do.md\n---\n";
        let err = SkillManifest::parse(content).unwrap_err();
        match &err {
            SkillError::InvalidManifest { reason } => assert!(reason.contains("semver")),
            _ => panic!("expected InvalidManifest, got {err:?}"),
        }
    }

    #[test]
    fn test_reject_empty_steps() {
        let content = "---\nname: test\nversion: \"1.0.0\"\nsteps: []\n---\n";
        let err = SkillManifest::parse(content).unwrap_err();
        match &err {
            SkillError::InvalidManifest { reason } => assert!(reason.contains("steps")),
            _ => panic!("expected InvalidManifest, got {err:?}"),
        }
    }

    #[test]
    fn test_accept_prerelease_semver() {
        let content = "---\nname: test\nversion: \"1.0.0-beta.1\"\nsteps:\n  - steps/01-do.md\n---\n";
        let manifest = SkillManifest::parse(content).unwrap();
        assert_eq!(manifest.version(), "1.0.0-beta.1");
    }

    #[test]
    fn test_reject_no_frontmatter() {
        let content = "# Just markdown\nNo front-matter here.";
        let err = SkillManifest::parse(content).unwrap_err();
        assert!(matches!(err, SkillError::InvalidManifest { .. }));
    }

    #[test]
    fn test_reject_corrupt_yaml() {
        let content = "---\nname: [invalid yaml\n---\n";
        let err = SkillManifest::parse(content).unwrap_err();
        match &err {
            SkillError::InvalidManifest { reason } => assert!(reason.contains("YAML")),
            _ => panic!("expected InvalidManifest, got {err:?}"),
        }
    }
}
