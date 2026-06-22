use std::path::{Path, PathBuf};

use crate::error::{AppError, Result};

// ── Data model ────────────────────────────────────────────────────────────────

pub use crate::domain::{
    manifest::{IdentityHints, Item, Manifest},
    ownership::OwnershipState,
};

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
