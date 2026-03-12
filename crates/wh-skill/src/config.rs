use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::SkillError;
use crate::repository::SkillRepository;

/// A reference to a skill with a version pin, as declared in a `.wh` file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillRef {
    /// The skill name (must match a skill directory name in the repository).
    pub name: String,
    /// Version pin string. Formats:
    /// - Bare semver: `"1.0.0"` (resolves to git tag `v1.0.0`)
    /// - Branch: `"branch:main"`
    /// - Commit: `"commit:<sha>"`
    pub version: String,
}

/// The `skills` section of a `.wh` configuration file.
///
/// This struct represents only the skills configuration portion of a `.wh` file.
/// Full `.wh` file parsing is handled separately (Story 2.2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillsConfig {
    /// Path to the local git repository containing skills.
    pub skills_repo: PathBuf,
    /// List of skill references with version pins.
    pub skills: Vec<SkillRef>,
}

impl SkillsConfig {
    /// Parse a skills configuration from a YAML string.
    ///
    /// This parses a standalone YAML snippet representing the skills section,
    /// not a complete `.wh` file.
    pub fn parse(yaml: &str) -> Result<Self, SkillError> {
        let config: SkillsConfig =
            serde_yaml::from_str(yaml).map_err(|e| SkillError::InvalidManifest {
                reason: format!("failed to parse skills config: {e}"),
            })?;

        if config.skills.is_empty() {
            return Err(SkillError::InvalidManifest {
                reason: "skills list must not be empty".into(),
            });
        }

        for skill_ref in &config.skills {
            if skill_ref.name.trim().is_empty() {
                return Err(SkillError::InvalidManifest {
                    reason: "skill name must not be empty".into(),
                });
            }
            if skill_ref.version.trim().is_empty() {
                return Err(SkillError::InvalidManifest {
                    reason: format!("version must not be empty for skill '{}'", skill_ref.name),
                });
            }
        }

        Ok(config)
    }

    /// Validate that all declared skills exist in the repository at their pinned versions.
    pub fn validate_against_repo(&self, repo: &SkillRepository) -> Result<(), SkillError> {
        for skill_ref in &self.skills {
            let oid = repo.resolve_version(&skill_ref.version)?;
            let skills = repo.discover_at(oid)?;
            if !skills.iter().any(|s| s.dir_name == skill_ref.name) {
                return Err(SkillError::ManifestNotFound {
                    path: format!(
                        "{}/skill.md (at version '{}')",
                        skill_ref.name, skill_ref.version
                    ),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_config() {
        let yaml = r#"
skills_repo: /path/to/skills
skills:
  - name: summarize
    version: "1.0.0"
  - name: web-search
    version: "branch:main"
"#;
        let config = SkillsConfig::parse(yaml).unwrap();
        assert_eq!(config.skills_repo, PathBuf::from("/path/to/skills"));
        assert_eq!(config.skills.len(), 2);
        assert_eq!(config.skills[0].name, "summarize");
        assert_eq!(config.skills[0].version, "1.0.0");
        assert_eq!(config.skills[1].name, "web-search");
        assert_eq!(config.skills[1].version, "branch:main");
    }

    #[test]
    fn test_parse_empty_skills() {
        let yaml = "skills_repo: /path\nskills: []\n";
        let err = SkillsConfig::parse(yaml).unwrap_err();
        assert!(matches!(err, SkillError::InvalidManifest { .. }));
    }

    #[test]
    fn test_parse_empty_skill_name() {
        let yaml = r#"
skills_repo: /path
skills:
  - name: ""
    version: "1.0.0"
"#;
        let err = SkillsConfig::parse(yaml).unwrap_err();
        match &err {
            SkillError::InvalidManifest { reason } => assert!(reason.contains("name")),
            _ => panic!("expected InvalidManifest, got {err:?}"),
        }
    }

    #[test]
    fn test_parse_empty_version() {
        let yaml = r#"
skills_repo: /path
skills:
  - name: test
    version: ""
"#;
        let err = SkillsConfig::parse(yaml).unwrap_err();
        match &err {
            SkillError::InvalidManifest { reason } => assert!(reason.contains("version")),
            _ => panic!("expected InvalidManifest, got {err:?}"),
        }
    }

    #[test]
    fn test_parse_invalid_yaml() {
        let yaml = "not: [valid: yaml";
        let err = SkillsConfig::parse(yaml).unwrap_err();
        assert!(matches!(err, SkillError::InvalidManifest { .. }));
    }

    #[test]
    fn test_parse_commit_version_format() {
        let yaml = r#"
skills_repo: /path/to/repo
skills:
  - name: my-skill
    version: "commit:abc123def456"
"#;
        let config = SkillsConfig::parse(yaml).unwrap();
        assert_eq!(config.skills[0].version, "commit:abc123def456");
    }
}
