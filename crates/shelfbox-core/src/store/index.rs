use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

// ── Data model ────────────────────────────────────────────────────────────────

/// A single entry in the global index, describing one known repository.
///
/// Fields that are environment-specific (absolute paths) live in the
/// local-state index rather than the store manifest so they are not
/// accidentally synced across machines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoEntry {
    /// Absolute path to the repository root on this machine.
    pub root: PathBuf,

    /// Absolute path to the `.git` directory (may differ from `root/.git`
    /// for worktrees).
    pub git_dir: PathBuf,

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

    /// Finds the repo ID whose root path matches `root`.
    pub fn find_by_root(&self, root: &Path) -> Option<&str> {
        self.repos
            .iter()
            .find_map(|(id, e)| (e.root == root).then_some(id.as_str()))
    }
}

// ── I/O ───────────────────────────────────────────────────────────────────────

/// Returns the path to the global index file.
///
/// `store_root` is the top-level store directory (e.g. `~/.local/share/shelfbox`).
pub fn index_path(store_root: &Path) -> PathBuf {
    store_root.join("index.json")
}

/// Reads and parses the index from disk.
///
/// If the file does not exist an empty [`Index`] is returned (first-run case).
pub fn load(store_root: &Path) -> Result<Index> {
    let path = index_path(store_root);
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| AppError::json(path, e)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Index::new()),
        Err(e) => Err(AppError::io(path, e)),
    }
}

/// Serialises and atomically writes the index to disk.
///
/// Uses a temp-file-then-rename strategy so a crash mid-write cannot
/// leave a corrupt index.
pub fn save(store_root: &Path, index: &Index) -> Result<()> {
    let path = index_path(store_root);

    // Ensure the parent directory exists.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    let json = serde_json::to_string_pretty(index).map_err(|e| AppError::json(path.clone(), e))?;

    // Write to a sibling temp file and rename for atomicity.
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, &json).map_err(|e| AppError::io(tmp_path.clone(), e))?;
    std::fs::rename(&tmp_path, &path).map_err(|e| AppError::io(path, e))?;

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_entry(root: &str) -> RepoEntry {
        RepoEntry {
            root: PathBuf::from(root),
            git_dir: PathBuf::from(format!("{root}/.git")),
            last_seen_at: "2026-04-29T00:00:00Z".into(),
        }
    }

    #[test]
    fn round_trip_empty_index() {
        let dir = TempDir::new().unwrap();
        let index = Index::new();
        save(dir.path(), &index).unwrap();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.repos.len(), 0);
    }

    #[test]
    fn round_trip_with_entry() {
        let dir = TempDir::new().unwrap();
        let mut index = Index::new();
        index.upsert(
            "01JWPQ3VKGE93V9BDHAENVXFA5",
            sample_entry("/home/user/myapp"),
        );

        save(dir.path(), &index).unwrap();
        let loaded = load(dir.path()).unwrap();

        let entry = loaded.get("01JWPQ3VKGE93V9BDHAENVXFA5").unwrap();
        assert_eq!(entry.root, PathBuf::from("/home/user/myapp"));
    }

    #[test]
    fn missing_file_returns_empty_index() {
        let dir = TempDir::new().unwrap();
        let index = load(dir.path()).unwrap();
        assert_eq!(index.repos.len(), 0);
    }

    #[test]
    fn find_by_root_returns_correct_id() {
        let mut index = Index::new();
        let id = "01JWPQ3VKGE93V9BDHAENVXFA5";
        index.upsert(id, sample_entry("/home/user/myapp"));

        let found = index.find_by_root(Path::new("/home/user/myapp"));
        assert_eq!(found, Some(id));
    }

    #[test]
    fn upsert_overwrites_existing_entry() {
        let dir = TempDir::new().unwrap();
        let id = "01JWPQ3VKGE93V9BDHAENVXFA5";
        let mut index = Index::new();
        index.upsert(id, sample_entry("/old/path"));
        index.upsert(id, sample_entry("/new/path"));

        save(dir.path(), &index).unwrap();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.get(id).unwrap().root, PathBuf::from("/new/path"));
    }
}
