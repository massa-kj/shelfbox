use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

// ── Data model ────────────────────────────────────────────────────────────────

/// The kind of filesystem object that was shelved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    File,
    Directory,
}

/// The type of link used to connect the repo path to the store item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkType {
    Symlink,
}

/// Link metadata stored per item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkInfo {
    /// The mechanism used to create the link.
    #[serde(rename = "type")]
    pub link_type: LinkType,
}

/// Git metadata recorded at the time the item was shelved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitInfo {
    /// Whether the file was tracked by git before shelving. Should almost
    /// always be `false` in normal usage; stored for auditability.
    pub was_tracked: bool,
}

/// A single shelved item recorded in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// Path relative to the repository root (forward slashes, no leading `/`).
    pub path: String,

    /// Path of the store-side file relative to the repo's store directory
    /// (i.e. relative to `repos/<id>/`).
    pub store_path: String,

    /// Whether this is a file or directory.
    pub kind: ItemKind,

    /// Link information.
    pub link: LinkInfo,

    /// Git metadata at the time of shelving.
    pub git: GitInfo,

    /// ISO-8601 creation timestamp.
    pub created_at: String,

    /// ISO-8601 last-updated timestamp.
    pub updated_at: String,
}

/// Repo metadata embedded in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoMeta {
    /// ULID repo identifier.
    pub id: String,

    /// Human-readable repository name (directory name of repo root).
    pub name: String,

    /// Remote URL of `origin`, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

/// The in-memory representation of `manifest.json` for a single repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    version: u32,

    /// Stable repo metadata (not environment-specific).
    pub repo: RepoMeta,

    /// All currently shelved items.
    pub items: Vec<Item>,
}

impl Manifest {
    const CURRENT_VERSION: u32 = 1;

    /// Creates a new, empty manifest for the given repository.
    pub fn new(meta: RepoMeta) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            repo: meta,
            items: Vec::new(),
        }
    }

    /// Returns the item whose repo-relative path matches `path`, if any.
    pub fn get(&self, path: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.path == path)
    }

    /// Returns `true` if `path` is already recorded in this manifest.
    pub fn contains(&self, path: &str) -> bool {
        self.get(path).is_some()
    }

    /// Appends a new item.  Panics in debug builds if `path` already exists
    /// (callers are responsible for checking [`contains`] first).
    pub fn add(&mut self, item: Item) {
        debug_assert!(
            !self.contains(&item.path),
            "item '{}' already in manifest",
            item.path
        );
        self.items.push(item);
    }

    /// Removes the item with the given `path`.  Returns `true` if an item
    /// was actually removed.
    pub fn remove(&mut self, path: &str) -> bool {
        let before = self.items.len();
        self.items.retain(|i| i.path != path);
        self.items.len() < before
    }
}

// ── I/O ───────────────────────────────────────────────────────────────────────

/// Returns the path to the manifest file for a given repo store directory.
///
/// `repo_store` is `<store_root>/repos/<repo_id>/`.
pub fn manifest_path(repo_store: &Path) -> PathBuf {
    repo_store.join("manifest.json")
}

/// Reads and parses the manifest from disk.
pub fn load(repo_store: &Path) -> Result<Manifest> {
    let path = manifest_path(repo_store);
    let s = std::fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))?;
    serde_json::from_str(&s).map_err(|e| AppError::json(path, e))
}

/// Serialises and atomically writes the manifest to disk.
pub fn save(repo_store: &Path, manifest: &Manifest) -> Result<()> {
    let path = manifest_path(repo_store);

    // Ensure the parent directory exists with restricted permissions so that
    // other users on the same machine cannot read shelved secrets.
    if let Some(parent) = path.parent() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)
                .map_err(|e| AppError::io(parent, e))?;
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    let json =
        serde_json::to_string_pretty(manifest).map_err(|e| AppError::json(path.clone(), e))?;

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

    fn sample_meta(id: &str) -> RepoMeta {
        RepoMeta {
            id: id.to_string(),
            name: "myapp".into(),
            remote: Some("git@github.com:example/myapp.git".into()),
        }
    }

    fn sample_item(path: &str) -> Item {
        Item {
            path: path.to_string(),
            store_path: format!("items/{path}"),
            kind: ItemKind::File,
            link: LinkInfo {
                link_type: LinkType::Symlink,
            },
            git: GitInfo { was_tracked: false },
            created_at: "2026-04-29T00:00:00Z".into(),
            updated_at: "2026-04-29T00:00:00Z".into(),
        }
    }

    #[test]
    fn round_trip_empty_manifest() {
        let dir = TempDir::new().unwrap();
        let manifest = Manifest::new(sample_meta("01JWPQ3VKGE93V9BDHAENVXFA5"));
        save(dir.path(), &manifest).unwrap();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.items.len(), 0);
        assert_eq!(loaded.repo.name, "myapp");
    }

    #[test]
    fn round_trip_with_item() {
        let dir = TempDir::new().unwrap();
        let mut manifest = Manifest::new(sample_meta("01JWPQ3VKGE93V9BDHAENVXFA5"));
        manifest.add(sample_item("notes/design.md"));

        save(dir.path(), &manifest).unwrap();
        let loaded = load(dir.path()).unwrap();

        assert_eq!(loaded.items.len(), 1);
        assert_eq!(loaded.items[0].path, "notes/design.md");
    }

    #[test]
    fn add_then_remove_item() {
        let mut manifest = Manifest::new(sample_meta("01JWPQ3VKGE93V9BDHAENVXFA5"));
        manifest.add(sample_item("notes.md"));
        assert!(manifest.contains("notes.md"));

        let removed = manifest.remove("notes.md");
        assert!(removed);
        assert!(!manifest.contains("notes.md"));
    }

    #[test]
    fn remove_nonexistent_returns_false() {
        let mut manifest = Manifest::new(sample_meta("01JWPQ3VKGE93V9BDHAENVXFA5"));
        assert!(!manifest.remove("ghost.md"));
    }

    #[test]
    fn remove_only_target_item() {
        let mut manifest = Manifest::new(sample_meta("01JWPQ3VKGE93V9BDHAENVXFA5"));
        manifest.add(sample_item("a.md"));
        manifest.add(sample_item("b.md"));
        manifest.remove("a.md");
        assert!(!manifest.contains("a.md"));
        assert!(manifest.contains("b.md"));
    }
}
