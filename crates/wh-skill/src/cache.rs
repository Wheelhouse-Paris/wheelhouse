//! Session-scoped skill cache for lazy loading.
//!
//! The `SkillCache` stores `LoadedSkill` instances keyed by `(skill_name, version_oid)`,
//! enabling on-demand fetching from git with subsequent invocations served from cache.

use std::collections::HashMap;

use git2::Oid;

use crate::repository::LoadedSkill;

/// A session-scoped cache for loaded skills.
///
/// Skills are keyed by `(skill_name, version_oid)` so that different versions
/// of the same skill are cached independently.
///
/// The cache starts empty and is populated lazily as skills are fetched from
/// git on demand. There is no eviction policy or maximum size — the cache
/// lives for the duration of the owning `InvocationPipeline`.
#[derive(Debug)]
pub struct SkillCache {
    entries: HashMap<(String, Oid), LoadedSkill>,
}

impl SkillCache {
    /// Create a new, empty skill cache.
    pub fn new() -> Self {
        SkillCache {
            entries: HashMap::new(),
        }
    }

    /// Look up a cached skill by name and version OID.
    ///
    /// Returns `None` if the skill is not in the cache (cache miss).
    pub fn get(&self, skill_name: &str, oid: Oid) -> Option<&LoadedSkill> {
        self.entries.get(&(skill_name.to_string(), oid))
    }

    /// Insert a loaded skill into the cache.
    ///
    /// If a skill with the same name and OID already exists, it is replaced.
    pub fn insert(&mut self, skill_name: &str, oid: Oid, skill: LoadedSkill) {
        self.entries.insert((skill_name.to_string(), oid), skill);
    }

    /// Check whether a skill is already cached.
    pub fn contains(&self, skill_name: &str, oid: Oid) -> bool {
        self.entries.contains_key(&(skill_name.to_string(), oid))
    }

    /// Remove all entries from the cache.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Return the number of cached skills.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for SkillCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directory::SkillStep;
    use crate::manifest::{SkillManifest, SkillManifestFrontMatter};

    fn make_loaded_skill(name: &str, version: &str) -> LoadedSkill {
        LoadedSkill {
            dir_name: name.to_string(),
            manifest: SkillManifest {
                front_matter: SkillManifestFrontMatter {
                    name: name.to_string(),
                    version: version.to_string(),
                    description: None,
                    inputs: vec![],
                    outputs: vec![],
                    steps: vec!["steps/01-do.md".into()],
                },
                body: String::new(),
            },
            steps: vec![SkillStep {
                filename: "01-do.md".into(),
                content: format!("Content for {name} v{version}"),
            }],
        }
    }

    /// Helper: create a deterministic Oid from a byte.
    fn oid_from_byte(b: u8) -> Oid {
        let mut bytes = [0u8; 20];
        bytes[0] = b;
        Oid::from_bytes(&bytes).unwrap()
    }

    #[test]
    fn new_cache_is_empty() {
        let cache = SkillCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn insert_and_get() {
        let mut cache = SkillCache::new();
        let oid = oid_from_byte(1);
        let skill = make_loaded_skill("summarize", "1.0.0");

        cache.insert("summarize", oid, skill);

        assert_eq!(cache.len(), 1);
        assert!(!cache.is_empty());

        let cached = cache.get("summarize", oid);
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().dir_name, "summarize");
    }

    #[test]
    fn cache_miss_returns_none() {
        let cache = SkillCache::new();
        let oid = oid_from_byte(1);
        assert!(cache.get("nonexistent", oid).is_none());
    }

    #[test]
    fn contains_check() {
        let mut cache = SkillCache::new();
        let oid = oid_from_byte(1);
        let skill = make_loaded_skill("summarize", "1.0.0");

        assert!(!cache.contains("summarize", oid));
        cache.insert("summarize", oid, skill);
        assert!(cache.contains("summarize", oid));
    }

    #[test]
    fn different_versions_cached_separately() {
        let mut cache = SkillCache::new();
        let oid_v1 = oid_from_byte(1);
        let oid_v2 = oid_from_byte(2);
        let skill_v1 = make_loaded_skill("summarize", "1.0.0");
        let skill_v2 = make_loaded_skill("summarize", "2.0.0");

        cache.insert("summarize", oid_v1, skill_v1);
        cache.insert("summarize", oid_v2, skill_v2);

        assert_eq!(cache.len(), 2);

        let v1 = cache.get("summarize", oid_v1).unwrap();
        assert!(v1.steps[0].content.contains("v1.0.0"));

        let v2 = cache.get("summarize", oid_v2).unwrap();
        assert!(v2.steps[0].content.contains("v2.0.0"));
    }

    #[test]
    fn different_skills_cached_independently() {
        let mut cache = SkillCache::new();
        let oid = oid_from_byte(1);
        let skill_a = make_loaded_skill("summarize", "1.0.0");
        let skill_b = make_loaded_skill("web-search", "1.0.0");

        cache.insert("summarize", oid, skill_a);
        cache.insert("web-search", oid, skill_b);

        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get("summarize", oid).unwrap().dir_name, "summarize");
        assert_eq!(cache.get("web-search", oid).unwrap().dir_name, "web-search");
    }

    #[test]
    fn clear_removes_all_entries() {
        let mut cache = SkillCache::new();
        let oid = oid_from_byte(1);
        cache.insert("summarize", oid, make_loaded_skill("summarize", "1.0.0"));
        cache.insert("web-search", oid, make_loaded_skill("web-search", "1.0.0"));

        assert_eq!(cache.len(), 2);
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn insert_replaces_existing() {
        let mut cache = SkillCache::new();
        let oid = oid_from_byte(1);
        let skill_old = make_loaded_skill("summarize", "1.0.0");
        let skill_new = make_loaded_skill("summarize", "1.0.1");

        cache.insert("summarize", oid, skill_old);
        cache.insert("summarize", oid, skill_new);

        assert_eq!(cache.len(), 1);
        let cached = cache.get("summarize", oid).unwrap();
        assert!(cached.steps[0].content.contains("v1.0.1"));
    }
}
