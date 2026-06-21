use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

// ── Data model ────────────────────────────────────────────────────────────────

pub use crate::domain::{manifest::IdentityHints, ownership::OwnershipState};

/// A single shelved item recorded in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// Immutable ULID assigned when this item is first shelved.
    pub item_id: String,

    /// Repository identity that originally shelved this item. Immutable.
    pub origin_repo_id: String,

    /// Path relative to the repository root (forward slashes, no leading `/`).
    pub path: String,

    /// Path of the store-side file relative to the repo's store directory.
    pub store_path: String,

    /// Current ownership state.
    pub ownership_state: OwnershipState,

    /// ISO-8601 creation timestamp.
    pub created_at: String,

    /// ISO-8601 last-updated timestamp.
    pub updated_at: String,
}

/// The in-memory representation of `manifest.json` for a single repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    version: u32,

    /// Stable repository identity.
    pub repo_id: String,

    /// ISO-8601 creation timestamp.
    pub created_at: String,

    /// Candidate-ranking and display hints.
    pub identity_hints: IdentityHints,

    /// All currently shelved items.
    pub items: Vec<Item>,
}

impl Manifest {
    pub const CURRENT_VERSION: u32 = 3;
    const MAX_REPO_NAME_HINTS: usize = 5;

    /// Creates a new, empty manifest for the given repository.
    pub fn new(repo_id: impl Into<String>, created_at: impl Into<String>) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            repo_id: repo_id.into(),
            created_at: created_at.into(),
            identity_hints: IdentityHints::default(),
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

    /// Appends a new item. Panics in debug builds if `path` already exists.
    pub fn add(&mut self, item: Item) {
        debug_assert!(
            !self.contains(&item.path),
            "item '{}' already in manifest",
            item.path
        );
        self.items.push(item);
    }

    /// Removes the item with the given `path`. Returns `true` if removed.
    pub fn remove(&mut self, path: &str) -> bool {
        let before = self.items.len();
        self.items.retain(|i| i.path != path);
        self.items.len() < before
    }

    /// Sets the ownership state of the item at `path`, updating `updated_at`.
    pub fn set_ownership_state(
        &mut self,
        path: &str,
        state: OwnershipState,
        updated_at: &str,
    ) -> bool {
        if let Some(item) = self.items.iter_mut().find(|i| i.path == path) {
            item.ownership_state = state;
            item.updated_at = updated_at.to_string();
            true
        } else {
            false
        }
    }

    /// Renames a manifest item in-place.
    pub fn rename(
        &mut self,
        old_path: &str,
        new_path: &str,
        new_store_path: &str,
        updated_at: &str,
    ) -> bool {
        if let Some(item) = self.items.iter_mut().find(|i| i.path == old_path) {
            item.path = new_path.to_string();
            item.store_path = new_store_path.to_string();
            item.updated_at = updated_at.to_string();
            true
        } else {
            false
        }
    }

    /// Adds a normalized remote hint if not already present.
    pub fn add_remote_hint(&mut self, hint: &str) {
        if hint.is_empty() {
            return;
        }
        if !self.identity_hints.remote_hints.iter().any(|h| h == hint) {
            self.identity_hints.remote_hints.push(hint.to_string());
        }
    }

    /// Adds a repository-name hint, most recent first.
    pub fn add_repo_name_hint(&mut self, name: &str) {
        if name.is_empty() {
            return;
        }
        self.identity_hints.repo_name_hints.retain(|h| h != name);
        self.identity_hints
            .repo_name_hints
            .insert(0, name.to_string());
        self.identity_hints
            .repo_name_hints
            .truncate(Self::MAX_REPO_NAME_HINTS);
    }

    /// Updates `last_attached_at`.
    pub fn touch_attached_at(&mut self, now: impl Into<String>) {
        self.identity_hints.last_attached_at = Some(now.into());
    }
}

// ── I/O ───────────────────────────────────────────────────────────────────────

/// Returns the path to the manifest file for a given repo store directory.
pub fn manifest_path(repo_store: &Path) -> PathBuf {
    repo_store.join("manifest.json")
}

pub fn read_version(repo_store: &Path) -> Result<u32> {
    let path = manifest_path(repo_store);
    let s = std::fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))?;
    let raw: serde_json::Value =
        serde_json::from_str(&s).map_err(|e| AppError::json(path.clone(), e))?;
    Ok(raw.get("version").and_then(|v| v.as_u64()).unwrap_or(0) as u32)
}

/// Reads and parses the manifest from disk.
pub fn load(repo_store: &Path) -> Result<Manifest> {
    let path = manifest_path(repo_store);
    let s = std::fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))?;

    let raw: serde_json::Value =
        serde_json::from_str(&s).map_err(|e| AppError::json(path.clone(), e))?;
    let found_version = raw.get("version").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    if found_version < Manifest::CURRENT_VERSION {
        return Err(AppError::ManifestVersionMismatch {
            path,
            found: found_version,
            expected: Manifest::CURRENT_VERSION,
        });
    }
    if found_version > Manifest::CURRENT_VERSION {
        return Err(AppError::ManifestVersionMismatch {
            path,
            found: found_version,
            expected: Manifest::CURRENT_VERSION,
        });
    }

    serde_json::from_value(raw).map_err(|e| AppError::json(path, e))
}

/// Serialises and atomically writes the manifest to disk.
pub fn save(repo_store: &Path, manifest: &Manifest) -> Result<()> {
    let path = manifest_path(repo_store);

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
    use ulid::Ulid;

    fn sample_manifest() -> Manifest {
        Manifest::new("01JWPQ3VKGE93V9BDHAENVXFA5", "2026-04-29T00:00:00Z")
    }

    fn sample_item(path: &str) -> Item {
        Item {
            item_id: Ulid::new().to_string(),
            origin_repo_id: "01JWPQ3VKGE93V9BDHAENVXFA5".into(),
            path: path.to_string(),
            store_path: format!("items/{path}"),
            ownership_state: OwnershipState::Attached,
            created_at: "2026-04-29T00:00:00Z".into(),
            updated_at: "2026-04-29T00:00:00Z".into(),
        }
    }

    #[test]
    fn round_trip_empty_manifest() {
        let dir = TempDir::new().unwrap();
        let manifest = sample_manifest();
        save(dir.path(), &manifest).unwrap();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.items.len(), 0);
        assert_eq!(loaded.repo_id, "01JWPQ3VKGE93V9BDHAENVXFA5");
    }

    #[test]
    fn round_trip_with_item_and_hints() {
        let dir = TempDir::new().unwrap();
        let mut manifest = sample_manifest();
        manifest.add_remote_hint("github.com/example/myapp");
        manifest.add_repo_name_hint("myapp");
        manifest.touch_attached_at("2026-05-01T00:00:00Z");
        manifest.add(sample_item("notes/design.md"));

        save(dir.path(), &manifest).unwrap();
        let loaded = load(dir.path()).unwrap();

        assert_eq!(loaded.items.len(), 1);
        assert_eq!(loaded.items[0].path, "notes/design.md");
        assert_eq!(
            loaded.identity_hints.remote_hints,
            vec!["github.com/example/myapp"]
        );
    }

    #[test]
    fn reject_manifest_with_missing_repo_id() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            manifest_path(dir.path()),
            r#"{"version":3,"created_at":"2026-04-29T00:00:00Z","identity_hints":{},"items":[]}"#,
        )
        .unwrap();

        assert!(load(dir.path()).is_err());
    }

    #[test]
    fn reject_manifest_with_invalid_ownership_state() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            manifest_path(dir.path()),
            r#"{
              "version":3,
              "repo_id":"01JWPQ3VKGE93V9BDHAENVXFA5",
              "created_at":"2026-04-29T00:00:00Z",
              "identity_hints":{},
              "items":[{
                "item_id":"01JWPQ3VKGE93V9BDHAENVXFA6",
                "origin_repo_id":"01JWPQ3VKGE93V9BDHAENVXFA5",
                "path":".env",
                "store_path":"items/.env",
                "ownership_state":"stale",
                "created_at":"2026-04-29T00:00:00Z",
                "updated_at":"2026-04-29T00:00:00Z"
              }]
            }"#,
        )
        .unwrap();

        assert!(load(dir.path()).is_err());
    }

    #[test]
    fn reject_manifest_below_version_3() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            manifest_path(dir.path()),
            r#"{"version":2,"repo_id":"01JWPQ3VKGE93V9BDHAENVXFA5","created_at":"2026-04-29T00:00:00Z","identity_hints":{},"items":[]}"#,
        )
        .unwrap();

        assert!(matches!(
            load(dir.path()),
            Err(AppError::ManifestVersionMismatch {
                found: 2,
                expected: 3,
                ..
            })
        ));
    }

    #[test]
    fn add_repo_name_hint_trims_dedupes_and_keeps_most_recent_first() {
        let mut manifest = sample_manifest();
        for name in ["a", "b", "c", "d", "e", "f"] {
            manifest.add_repo_name_hint(name);
        }
        manifest.add_repo_name_hint("d");

        assert_eq!(
            manifest.identity_hints.repo_name_hints,
            vec!["d", "f", "e", "c", "b"]
        );
    }

    #[test]
    fn add_remote_hint_deduplicates_without_reordering() {
        let mut manifest = sample_manifest();
        manifest.add_remote_hint("github.com/example/a");
        manifest.add_remote_hint("github.com/example/b");
        manifest.add_remote_hint("github.com/example/a");

        assert_eq!(
            manifest.identity_hints.remote_hints,
            vec!["github.com/example/a", "github.com/example/b"]
        );
    }

    #[test]
    fn add_then_remove_item() {
        let mut manifest = sample_manifest();
        manifest.add(sample_item("notes.md"));
        assert!(manifest.contains("notes.md"));

        let removed = manifest.remove("notes.md");
        assert!(removed);
        assert!(!manifest.contains("notes.md"));
    }

    #[test]
    fn rename_updates_path_and_store_path() {
        let mut manifest = sample_manifest();
        manifest.add(sample_item("old/file.md"));

        let renamed = manifest.rename(
            "old/file.md",
            "new/file.md",
            "items/new/file.md",
            "2026-05-25T00:00:00Z",
        );

        assert!(renamed);
        assert!(!manifest.contains("old/file.md"));
        let item = manifest.get("new/file.md").expect("new path must exist");
        assert_eq!(item.store_path, "items/new/file.md");
        assert_eq!(item.updated_at, "2026-05-25T00:00:00Z");
        assert_eq!(item.created_at, "2026-04-29T00:00:00Z");
    }
}
