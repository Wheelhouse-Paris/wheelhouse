use std::path::{Path, PathBuf};

use git2::{Oid, Repository};

use crate::directory::SkillStep;
use crate::error::SkillError;
use crate::manifest::SkillManifest;

/// A discovered skill entry from the git repository (not yet fully loaded).
#[derive(Debug, Clone)]
pub struct SkillEntry {
    /// Skill directory name (e.g., "summarize").
    pub dir_name: String,
    /// The parsed manifest.
    pub manifest: SkillManifest,
}

/// A fully loaded skill from the git repository, including step contents.
#[derive(Debug, Clone)]
pub struct LoadedSkill {
    /// Skill directory name.
    pub dir_name: String,
    /// The parsed manifest.
    pub manifest: SkillManifest,
    /// Loaded step files in order.
    pub steps: Vec<SkillStep>,
}

/// A git-based skill repository for discovering and loading skills.
///
/// Skills are stored in the repository root as directories, each containing
/// a `skill.md` manifest and a `steps/` subdirectory.
pub struct SkillRepository {
    /// Path to the local git repository.
    path: PathBuf,
    /// The opened git2 repository handle.
    repo: Repository,
}

impl SkillRepository {
    /// Open a local git repository at the given path.
    pub fn open(path: &Path) -> Result<Self, SkillError> {
        let repo = Repository::open(path)?;
        Ok(SkillRepository {
            path: path.to_path_buf(),
            repo,
        })
    }

    /// Get the repository path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Resolve a version pin string to a git object ID.
    ///
    /// Supported formats:
    /// - Bare semver (e.g., `"1.0.0"`) -> resolves to tag `v1.0.0`
    /// - `"branch:<name>"` -> resolves to branch tip
    /// - `"commit:<sha>"` -> resolves to exact commit
    pub fn resolve_version(&self, version: &str) -> Result<Oid, SkillError> {
        if let Some(branch_name) = version.strip_prefix("branch:") {
            // Resolve branch name to its tip commit
            let branch = self
                .repo
                .find_branch(branch_name, git2::BranchType::Local)
                .map_err(|_| SkillError::VersionNotFound {
                    version: version.to_string(),
                })?;
            let commit =
                branch
                    .get()
                    .peel_to_commit()
                    .map_err(|_| SkillError::VersionNotFound {
                        version: version.to_string(),
                    })?;
            Ok(commit.id())
        } else if let Some(sha) = version.strip_prefix("commit:") {
            let oid = Oid::from_str(sha).map_err(|_| SkillError::VersionNotFound {
                version: version.to_string(),
            })?;
            // Verify the commit exists
            self.repo
                .find_commit(oid)
                .map_err(|_| SkillError::VersionNotFound {
                    version: version.to_string(),
                })?;
            Ok(oid)
        } else {
            // Bare semver -> tag v<version>
            let tag_name = format!("v{version}");
            let refname = format!("refs/tags/{tag_name}");
            let reference =
                self.repo
                    .find_reference(&refname)
                    .map_err(|_| SkillError::VersionNotFound {
                        version: version.to_string(),
                    })?;
            let commit = reference
                .peel_to_commit()
                .map_err(|_| SkillError::VersionNotFound {
                    version: version.to_string(),
                })?;
            Ok(commit.id())
        }
    }

    /// Discover all skills at a specific git ref (commit OID).
    ///
    /// Walks the tree at the given commit and finds directories containing `skill.md`.
    pub fn discover_at(&self, oid: Oid) -> Result<Vec<SkillEntry>, SkillError> {
        let commit = self.repo.find_commit(oid)?;
        let tree = commit.tree()?;

        let mut skills = Vec::new();

        // Walk top-level entries only
        for entry in tree.iter() {
            let name = match entry.name() {
                Some(n) => n.to_string(),
                None => continue,
            };

            // Skip non-tree entries (files at root level)
            if entry.kind() != Some(git2::ObjectType::Tree) {
                continue;
            }

            // Check if this subtree contains skill.md
            let subtree = match self.repo.find_tree(entry.id()) {
                Ok(t) => t,
                Err(_) => continue,
            };

            let skill_md_entry = subtree.get_name("skill.md");
            if let Some(skill_md) = skill_md_entry {
                // Read the skill.md blob
                let blob = match self.repo.find_blob(skill_md.id()) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let content = match std::str::from_utf8(blob.content()) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                match SkillManifest::parse(content) {
                    Ok(manifest) => {
                        skills.push(SkillEntry {
                            dir_name: name,
                            manifest,
                        });
                    }
                    Err(_) => continue, // Skip invalid manifests
                }
            }
        }

        // Sort by directory name for deterministic output
        skills.sort_by(|a, b| a.dir_name.cmp(&b.dir_name));
        Ok(skills)
    }

    /// Load a specific skill at a specific git ref, including step file contents.
    ///
    /// Reads the skill.md manifest and all step files referenced in it
    /// directly from the git object database (no checkout needed).
    pub fn load_skill_at(&self, skill_dir_name: &str, oid: Oid) -> Result<LoadedSkill, SkillError> {
        let commit = self.repo.find_commit(oid)?;
        let tree = commit.tree()?;

        // Find the skill directory in the tree
        let skill_tree_entry =
            tree.get_name(skill_dir_name)
                .ok_or_else(|| SkillError::StepNotFound {
                    step: format!("{skill_dir_name}/"),
                })?;
        let skill_tree = self.repo.find_tree(skill_tree_entry.id())?;

        // Read skill.md
        let skill_md_entry =
            skill_tree
                .get_name("skill.md")
                .ok_or_else(|| SkillError::ManifestNotFound {
                    path: format!("{skill_dir_name}/skill.md"),
                })?;
        let skill_md_blob = self.repo.find_blob(skill_md_entry.id())?;
        let skill_md_content = std::str::from_utf8(skill_md_blob.content()).map_err(|_| {
            SkillError::InvalidManifest {
                reason: "skill.md is not valid UTF-8".into(),
            }
        })?;
        let manifest = SkillManifest::parse(skill_md_content)?;

        // Read step files from the manifest references
        let mut steps = Vec::new();
        for step_ref in manifest.steps() {
            // step_ref is like "steps/01-gather.md"
            let blob = self.read_blob_at_path(&skill_tree, step_ref)?;
            let content = std::str::from_utf8(&blob).map_err(|_| SkillError::InvalidManifest {
                reason: format!("step file {step_ref} is not valid UTF-8"),
            })?;
            let filename = Path::new(step_ref)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| step_ref.clone());
            steps.push(SkillStep {
                filename,
                content: content.to_string(),
            });
        }

        Ok(LoadedSkill {
            dir_name: skill_dir_name.to_string(),
            manifest,
            steps,
        })
    }

    /// Read a blob from a nested path within a tree (e.g., "steps/01-gather.md").
    fn read_blob_at_path(&self, root_tree: &git2::Tree, path: &str) -> Result<Vec<u8>, SkillError> {
        let entry = root_tree
            .get_path(Path::new(path))
            .map_err(|_| SkillError::StepNotFound {
                step: path.to_string(),
            })?;
        let blob = self
            .repo
            .find_blob(entry.id())
            .map_err(|_| SkillError::StepNotFound {
                step: path.to_string(),
            })?;
        Ok(blob.content().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::Signature;
    use std::fs;
    use tempfile::TempDir;

    /// Helper: create a temp git repo, add skill files, and commit.
    /// Returns (TempDir, Repository, Oid of commit).
    fn create_repo_with_skill(
        skill_name: &str,
        version: &str,
        step_names: &[&str],
    ) -> (TempDir, Oid) {
        let tmp = TempDir::new().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();

        // Create skill files on disk
        let skill_dir = tmp.path().join(skill_name);
        let steps_dir = skill_dir.join("steps");
        fs::create_dir_all(&steps_dir).unwrap();

        let step_refs: Vec<String> = step_names.iter().map(|s| format!("steps/{s}")).collect();
        let steps_yaml = step_refs
            .iter()
            .map(|s| format!("  - {s}"))
            .collect::<Vec<_>>()
            .join("\n");

        let manifest_content = format!(
            "---\nname: {skill_name}\nversion: \"{version}\"\nsteps:\n{steps_yaml}\n---\n\n# {skill_name}\n"
        );
        fs::write(skill_dir.join("skill.md"), &manifest_content).unwrap();

        for step_name in step_names {
            fs::write(
                steps_dir.join(step_name),
                format!("# Step: {step_name}\n\nContent for {step_name}.\n"),
            )
            .unwrap();
        }

        // Stage and commit
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = Signature::now("test", "test@example.com").unwrap();
        let oid = repo
            .commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
            .unwrap();

        (tmp, oid)
    }

    /// Helper: create a tag pointing to a commit.
    fn create_tag(repo_path: &Path, tag_name: &str, oid: Oid) {
        let repo = Repository::open(repo_path).unwrap();
        let commit = repo.find_commit(oid).unwrap();
        repo.tag_lightweight(tag_name, commit.as_object(), false)
            .unwrap();
    }

    #[test]
    fn test_open_repository() {
        let (tmp, _oid) = create_repo_with_skill("test-skill", "1.0.0", &["01-step.md"]);
        let repo = SkillRepository::open(tmp.path()).unwrap();
        assert_eq!(repo.path(), tmp.path());
    }

    #[test]
    fn test_discover_skills_at_ref() {
        let (tmp, oid) = create_repo_with_skill("summarize", "1.0.0", &["01-gather.md"]);
        let repo = SkillRepository::open(tmp.path()).unwrap();

        let skills = repo.discover_at(oid).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].dir_name, "summarize");
        assert_eq!(skills[0].manifest.name(), "summarize");
    }

    #[test]
    fn test_load_skill_at_ref() {
        let (tmp, oid) =
            create_repo_with_skill("web-search", "1.0.0", &["01-search.md", "02-format.md"]);
        let repo = SkillRepository::open(tmp.path()).unwrap();

        let loaded = repo.load_skill_at("web-search", oid).unwrap();
        assert_eq!(loaded.manifest.name(), "web-search");
        assert_eq!(loaded.steps.len(), 2);
        assert_eq!(loaded.steps[0].filename, "01-search.md");
        assert_eq!(loaded.steps[1].filename, "02-format.md");
        assert!(loaded.steps[0].content.contains("01-search.md"));
    }

    #[test]
    fn test_resolve_version_tag() {
        let (tmp, oid) = create_repo_with_skill("test", "1.0.0", &["01-do.md"]);
        create_tag(tmp.path(), "v1.0.0", oid);

        let repo = SkillRepository::open(tmp.path()).unwrap();
        let resolved = repo.resolve_version("1.0.0").unwrap();
        assert_eq!(resolved, oid);
    }

    #[test]
    fn test_resolve_version_branch() {
        let (tmp, oid) = create_repo_with_skill("test", "1.0.0", &["01-do.md"]);

        // The default branch should be resolvable
        let git_repo = Repository::open(tmp.path()).unwrap();
        let head = git_repo.head().unwrap();
        let branch_name = head.shorthand().unwrap().to_string();

        let repo = SkillRepository::open(tmp.path()).unwrap();
        let resolved = repo
            .resolve_version(&format!("branch:{branch_name}"))
            .unwrap();
        assert_eq!(resolved, oid);
    }

    #[test]
    fn test_resolve_version_commit() {
        let (tmp, oid) = create_repo_with_skill("test", "1.0.0", &["01-do.md"]);

        let repo = SkillRepository::open(tmp.path()).unwrap();
        let resolved = repo.resolve_version(&format!("commit:{oid}")).unwrap();
        assert_eq!(resolved, oid);
    }

    #[test]
    fn test_resolve_nonexistent_version() {
        let (tmp, _oid) = create_repo_with_skill("test", "1.0.0", &["01-do.md"]);
        let repo = SkillRepository::open(tmp.path()).unwrap();

        let err = repo.resolve_version("99.99.99").unwrap_err();
        assert!(matches!(err, SkillError::VersionNotFound { .. }));
    }

    #[test]
    fn test_version_pinning_returns_correct_content() {
        let tmp = TempDir::new().unwrap();
        let git_repo = Repository::init(tmp.path()).unwrap();
        let sig = Signature::now("test", "test@example.com").unwrap();

        // Create v1 skill
        let skill_dir = tmp.path().join("my-skill");
        let steps_dir = skill_dir.join("steps");
        fs::create_dir_all(&steps_dir).unwrap();
        fs::write(
            skill_dir.join("skill.md"),
            "---\nname: my-skill\nversion: \"1.0.0\"\nsteps:\n  - steps/01-do.md\n---\n\n# V1\n",
        )
        .unwrap();
        fs::write(steps_dir.join("01-do.md"), "# Step V1\nVersion 1 content.").unwrap();

        let mut index = git_repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = git_repo.find_tree(tree_oid).unwrap();
        let oid_v1 = git_repo
            .commit(Some("HEAD"), &sig, &sig, "v1", &tree, &[])
            .unwrap();
        let commit_v1 = git_repo.find_commit(oid_v1).unwrap();
        git_repo
            .tag_lightweight("v1.0.0", commit_v1.as_object(), false)
            .unwrap();

        // Create v2 skill (modify content)
        fs::write(
            skill_dir.join("skill.md"),
            "---\nname: my-skill\nversion: \"2.0.0\"\nsteps:\n  - steps/01-do.md\n---\n\n# V2\n",
        )
        .unwrap();
        fs::write(steps_dir.join("01-do.md"), "# Step V2\nVersion 2 content.").unwrap();

        let mut index = git_repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = git_repo.find_tree(tree_oid).unwrap();
        let oid_v2 = git_repo
            .commit(Some("HEAD"), &sig, &sig, "v2", &tree, &[&commit_v1])
            .unwrap();
        let commit_v2 = git_repo.find_commit(oid_v2).unwrap();
        git_repo
            .tag_lightweight("v2.0.0", commit_v2.as_object(), false)
            .unwrap();

        // Now open as SkillRepository and verify version pinning
        let repo = SkillRepository::open(tmp.path()).unwrap();

        // Load v1 — should get v1 content even though v2 is HEAD
        let v1_oid = repo.resolve_version("1.0.0").unwrap();
        let v1_skill = repo.load_skill_at("my-skill", v1_oid).unwrap();
        assert_eq!(v1_skill.manifest.version(), "1.0.0");
        assert!(v1_skill.steps[0].content.contains("Version 1"));

        // Load v2
        let v2_oid = repo.resolve_version("2.0.0").unwrap();
        let v2_skill = repo.load_skill_at("my-skill", v2_oid).unwrap();
        assert_eq!(v2_skill.manifest.version(), "2.0.0");
        assert!(v2_skill.steps[0].content.contains("Version 2"));
    }
}
