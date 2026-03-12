//! Skill allowlist validation (FM-05).
//!
//! The allowlist determines which skills an agent is permitted to invoke.
//! It is derived from the `.wh` skills configuration.

use std::collections::HashSet;

use crate::config::SkillsConfig;
use crate::error::SkillError;

/// A set of skill names that an agent is permitted to invoke.
///
/// Built from `.wh` configuration and checked before skill execution (FM-05).
#[derive(Debug, Clone)]
pub struct SkillAllowlist {
    /// Permitted skill names.
    allowed: HashSet<String>,
}

impl SkillAllowlist {
    /// Create a new allowlist from a list of permitted skill names.
    pub fn new(skills: Vec<String>) -> Self {
        SkillAllowlist {
            allowed: skills.into_iter().collect(),
        }
    }

    /// Build an allowlist from a `.wh` skills configuration.
    ///
    /// Extracts skill names from all `SkillRef` entries in the config.
    pub fn from_config(config: &SkillsConfig) -> Self {
        let names = config.skills.iter().map(|r| r.name.clone()).collect();
        Self::new(names)
    }

    /// Check if a skill name is in the allowlist.
    pub fn is_allowed(&self, skill_name: &str) -> bool {
        self.allowed.contains(skill_name)
    }

    /// Validate that a skill invocation is permitted.
    ///
    /// Returns `Ok(())` if the skill is allowed, or `SkillError::SkillNotPermitted`
    /// if the skill is not in the allowlist.
    pub fn validate(&self, skill_name: &str, agent_id: &str) -> Result<(), SkillError> {
        if self.is_allowed(skill_name) {
            Ok(())
        } else {
            Err(SkillError::SkillNotPermitted {
                skill_name: skill_name.to_string(),
                agent_id: agent_id.to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SkillRef;
    use std::path::PathBuf;

    #[test]
    fn allowed_skill_passes() {
        let list = SkillAllowlist::new(vec!["summarize".into(), "web-search".into()]);
        assert!(list.is_allowed("summarize"));
        assert!(list.is_allowed("web-search"));
    }

    #[test]
    fn disallowed_skill_rejected() {
        let list = SkillAllowlist::new(vec!["summarize".into()]);
        assert!(!list.is_allowed("web-search"));
    }

    #[test]
    fn empty_allowlist_rejects_all() {
        let list = SkillAllowlist::new(vec![]);
        assert!(!list.is_allowed("summarize"));
        assert!(!list.is_allowed("anything"));
    }

    #[test]
    fn validate_returns_error_for_disallowed() {
        let list = SkillAllowlist::new(vec!["summarize".into()]);
        let err = list.validate("web-search", "agent-1").unwrap_err();
        match err {
            SkillError::SkillNotPermitted {
                skill_name,
                agent_id,
            } => {
                assert_eq!(skill_name, "web-search");
                assert_eq!(agent_id, "agent-1");
            }
            _ => panic!("expected SkillNotPermitted, got {err:?}"),
        }
    }

    #[test]
    fn validate_returns_ok_for_allowed() {
        let list = SkillAllowlist::new(vec!["summarize".into()]);
        assert!(list.validate("summarize", "agent-1").is_ok());
    }

    #[test]
    fn from_config_extracts_skill_names() {
        let config = SkillsConfig {
            skills_repo: PathBuf::from("/path/to/repo"),
            skills: vec![
                SkillRef {
                    name: "summarize".into(),
                    version: "1.0.0".into(),
                },
                SkillRef {
                    name: "web-search".into(),
                    version: "branch:main".into(),
                },
            ],
        };
        let list = SkillAllowlist::from_config(&config);
        assert!(list.is_allowed("summarize"));
        assert!(list.is_allowed("web-search"));
        assert!(!list.is_allowed("unknown"));
    }
}
