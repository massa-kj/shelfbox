use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// A single entry in the global index, describing one known repository.
///
/// Fields that are environment-specific (absolute paths) live in the
/// local-state index rather than the store manifest, keeping `manifest.json`
/// portable across store relocations on the same machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoEntry {
    /// Absolute path to the repository root on this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,

    /// Absolute path to the `.git` directory (may differ from `root/.git`
    /// for worktrees).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_dir: Option<PathBuf>,

    /// Absolute path to the git-common-dir — the shared `.git/` directory
    /// that is stable across all linked worktrees of the same clone.
    /// Equivalent to `git rev-parse --git-common-dir`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_common_dir: Option<PathBuf>,

    /// Name of the per-repo directory under `<store>/repos/`.
    /// Format: `<sanitized-repo-name>`, with numeric suffixes for conflicts.
    pub repo_store_dir: String,

    /// ISO-8601 timestamp of the last time this repo was accessed via
    /// shelfbox.
    pub last_seen_at: String,
}

/// The in-memory representation of `index.json`.
///
/// The key is a ULID string (26 ASCII characters, Crockford base32).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Index {
    version: u32,
    repos: HashMap<String, RepoEntry>,
}

impl Index {
    const CURRENT_VERSION: u32 = 1;

    /// Create an empty index at the current schema version.
    pub fn new() -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            repos: HashMap::new(),
        }
    }

    /// Returns the entry for `repo_id`, if present.
    pub fn get(&self, repo_id: &str) -> Option<&RepoEntry> {
        self.repos.get(repo_id)
    }

    /// Inserts or replaces the entry for `repo_id`.
    pub fn upsert(&mut self, repo_id: impl Into<String>, entry: RepoEntry) {
        self.repos.insert(repo_id.into(), entry);
    }

    /// Returns an iterator over all `(id, entry)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &RepoEntry)> {
        self.repos.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Returns the number of indexed repositories.
    pub fn len(&self) -> usize {
        self.repos.len()
    }

    /// Returns true when no repositories are indexed.
    pub fn is_empty(&self) -> bool {
        self.repos.is_empty()
    }

    /// Finds the repo ID whose root path matches `root`.
    pub fn find_by_root(&self, root: &Path) -> Option<&str> {
        self.repos
            .iter()
            .find_map(|(id, e)| (e.root.as_deref() == Some(root)).then_some(id.as_str()))
    }

    /// Removes the entry for `repo_id`. Returns `true` if an entry was removed.
    pub fn remove(&mut self, repo_id: &str) -> bool {
        self.repos.remove(repo_id).is_some()
    }

    /// Finds the repo ID whose `git_common_dir` matches `common_dir`.
    ///
    /// This secondary lookup handles the case where a repository was accessed
    /// via a linked worktree (different `root`) but shares the same underlying
    /// git objects directory.
    pub fn find_by_git_common_dir(&self, common_dir: &Path) -> Option<&str> {
        self.repos.iter().find_map(|(id, e)| {
            (e.git_common_dir.as_deref() == Some(common_dir)).then_some(id.as_str())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_entry(root: &str) -> RepoEntry {
        RepoEntry {
            root: Some(PathBuf::from(root)),
            git_dir: Some(PathBuf::from(format!("{root}/.git"))),
            git_common_dir: Some(PathBuf::from(format!("{root}/.git"))),
            repo_store_dir: "myapp".into(),
            last_seen_at: "2026-04-29T00:00:00Z".into(),
        }
    }

    #[test]
    fn index_json_shape_matches_v1_schema() {
        let mut index = Index::new();
        index.upsert(
            "01JWPQ3VKGE93V9BDHAENVXFA5",
            sample_entry("/home/user/myapp"),
        );

        assert_eq!(
            serde_json::to_value(index).unwrap(),
            json!({
                "version": 1,
                "repos": {
                    "01JWPQ3VKGE93V9BDHAENVXFA5": {
                        "root": "/home/user/myapp",
                        "git_dir": "/home/user/myapp/.git",
                        "git_common_dir": "/home/user/myapp/.git",
                        "repo_store_dir": "myapp",
                        "last_seen_at": "2026-04-29T00:00:00Z"
                    }
                }
            })
        );
    }

    #[test]
    fn index_defaults_missing_local_git_fields() {
        let index: Index = serde_json::from_value(json!({
            "version": 1,
            "repos": {
                "01JWPQ3VKGE93V9BDHAENVXFA5": {
                    "repo_store_dir": "myapp",
                    "last_seen_at": "2026-04-29T00:00:00Z"
                }
            }
        }))
        .unwrap();
        let entry = index.get("01JWPQ3VKGE93V9BDHAENVXFA5").unwrap();

        assert_eq!(entry.repo_store_dir, "myapp");
        assert_eq!(entry.root, None);
        assert_eq!(entry.git_dir, None);
        assert_eq!(entry.git_common_dir, None);
    }

    #[test]
    fn index_lookup_prefers_exact_methods() {
        let mut index = Index::new();
        let id = "01JWPQ3VKGE93V9BDHAENVXFA5";
        index.upsert(id, sample_entry("/home/user/myapp"));

        assert_eq!(index.find_by_root(Path::new("/home/user/myapp")), Some(id));
        assert_eq!(
            index.find_by_git_common_dir(Path::new("/home/user/myapp/.git")),
            Some(id)
        );
        assert!(index.remove(id));
        assert_eq!(index.get(id), None);
    }
}
