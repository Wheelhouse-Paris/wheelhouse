use std::fs;
use std::path::Path;
use std::path::PathBuf;

use crate::error::SkillError;
use crate::manifest::SkillManifest;

/// A loaded step file with its name and content.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillStep {
    /// The filename (e.g., "01-gather-context.md").
    pub filename: String,
    /// The markdown content of the step.
    pub content: String,
}

/// Represents a skill loaded from a directory on the filesystem.
///
/// A skill directory has the canonical layout:
/// ```text
/// <skill-name>/
///   skill.md          # manifest (YAML front-matter + markdown)
///   steps/
///     01-first.md
///     02-second.md
/// ```
#[derive(Debug, Clone)]
pub struct SkillDirectory {
    /// The path to the skill directory.
    pub path: PathBuf,
    /// The parsed manifest from `skill.md`.
    pub manifest: SkillManifest,
    /// Loaded step files, ordered by numeric prefix.
    pub steps: Vec<SkillStep>,
}

impl SkillDirectory {
    /// Load a skill from a directory path.
    ///
    /// Reads `skill.md`, parses the manifest, then loads and orders
    /// all step files from the `steps/` subdirectory.
    pub fn load(path: &Path) -> Result<Self, SkillError> {
        let manifest_path = path.join("skill.md");
        if !manifest_path.exists() {
            return Err(SkillError::ManifestNotFound {
                path: manifest_path.display().to_string(),
            });
        }

        let content = fs::read_to_string(&manifest_path)?;
        let manifest = SkillManifest::parse(&content)?;

        // Load step files referenced in the manifest
        let mut steps = Vec::new();
        for step_ref in manifest.steps() {
            let step_path = path.join(step_ref);
            if !step_path.exists() {
                return Err(SkillError::StepNotFound {
                    step: step_ref.clone(),
                });
            }
            let step_content = fs::read_to_string(&step_path)?;
            let filename = step_path
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();
            steps.push(SkillStep {
                filename,
                content: step_content,
            });
        }

        Ok(SkillDirectory {
            path: path.to_path_buf(),
            manifest,
            steps,
        })
    }

    /// Scan a root directory and discover all valid skill directories.
    ///
    /// A subdirectory is considered a valid skill if it contains a `skill.md` file
    /// that can be parsed successfully. Directories without `skill.md` or with
    /// invalid manifests are silently skipped.
    pub fn discover(root: &Path) -> Result<Vec<SkillDirectory>, SkillError> {
        let mut skills = Vec::new();

        if !root.is_dir() {
            return Ok(skills);
        }

        let mut entries: Vec<_> = fs::read_dir(root)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();

        // Sort entries by name for deterministic ordering
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let dir_path = entry.path();
            if dir_path.join("skill.md").exists() {
                match SkillDirectory::load(&dir_path) {
                    Ok(skill) => skills.push(skill),
                    Err(_) => continue, // Skip invalid skills
                }
            }
        }

        Ok(skills)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_skill_dir(root: &Path, name: &str, version: &str, step_names: &[&str]) {
        let skill_dir = root.join(name);
        let steps_dir = skill_dir.join("steps");
        fs::create_dir_all(&steps_dir).unwrap();

        let step_refs: Vec<String> = step_names.iter().map(|s| format!("steps/{s}")).collect();
        let steps_yaml = step_refs
            .iter()
            .map(|s| format!("  - {s}"))
            .collect::<Vec<_>>()
            .join("\n");

        let manifest = format!(
            "---\nname: {name}\nversion: \"{version}\"\nsteps:\n{steps_yaml}\n---\n\n# {name}\n\nA test skill.\n"
        );
        fs::write(skill_dir.join("skill.md"), manifest).unwrap();

        for step_name in step_names {
            fs::write(
                steps_dir.join(step_name),
                format!("# Step: {step_name}\n\nDo something.\n"),
            )
            .unwrap();
        }
    }

    #[test]
    fn test_load_valid_skill_directory() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "summarize",
            "1.0.0",
            &["01-gather.md", "02-execute.md"],
        );

        let skill = SkillDirectory::load(&tmp.path().join("summarize")).unwrap();
        assert_eq!(skill.manifest.name(), "summarize");
        assert_eq!(skill.steps.len(), 2);
        assert_eq!(skill.steps[0].filename, "01-gather.md");
        assert_eq!(skill.steps[1].filename, "02-execute.md");
    }

    #[test]
    fn test_load_missing_manifest() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("empty-skill")).unwrap();

        let err = SkillDirectory::load(&tmp.path().join("empty-skill")).unwrap_err();
        assert!(matches!(err, SkillError::ManifestNotFound { .. }));
    }

    #[test]
    fn test_load_missing_step_file() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("broken");
        let steps_dir = skill_dir.join("steps");
        fs::create_dir_all(&steps_dir).unwrap();

        let manifest =
            "---\nname: broken\nversion: \"1.0.0\"\nsteps:\n  - steps/01-missing.md\n---\n";
        fs::write(skill_dir.join("skill.md"), manifest).unwrap();

        let err = SkillDirectory::load(&skill_dir).unwrap_err();
        assert!(matches!(err, SkillError::StepNotFound { .. }));
    }

    #[test]
    fn test_discover_skills_in_root() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(tmp.path(), "alpha", "1.0.0", &["01-do.md"]);
        create_skill_dir(tmp.path(), "beta", "2.0.0", &["01-step.md"]);

        // Also create a non-skill directory (no skill.md)
        fs::create_dir_all(tmp.path().join("not-a-skill")).unwrap();

        let skills = SkillDirectory::discover(tmp.path()).unwrap();
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].manifest.name(), "alpha");
        assert_eq!(skills[1].manifest.name(), "beta");
    }

    #[test]
    fn test_discover_empty_root() {
        let tmp = TempDir::new().unwrap();
        let skills = SkillDirectory::discover(tmp.path()).unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    fn test_step_ordering_preserved_from_manifest() {
        let tmp = TempDir::new().unwrap();
        // Create skill with steps in a specific order
        create_skill_dir(
            tmp.path(),
            "ordered",
            "1.0.0",
            &["01-first.md", "02-second.md", "03-third.md"],
        );

        let skill = SkillDirectory::load(&tmp.path().join("ordered")).unwrap();
        assert_eq!(skill.steps.len(), 3);
        assert_eq!(skill.steps[0].filename, "01-first.md");
        assert_eq!(skill.steps[1].filename, "02-second.md");
        assert_eq!(skill.steps[2].filename, "03-third.md");
    }
}
