use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::{
    context::now_iso8601,
    error::{AppError, Result},
};

// ── Data model ────────────────────────────────────────────────────────────────

/// Contents of `<store>/meta.json`.
///
/// Provides a stable identity for the store itself so that future sync
/// tooling can distinguish a "same store" clone from an independent store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreMeta {
    /// Globally unique ULID assigned when the store is first created.
    pub store_id: String,

    /// ISO-8601 timestamp when this store was initialised.
    pub created_at: String,
}

// ── Path helper ───────────────────────────────────────────────────────────────

fn meta_path(store_root: &Path) -> PathBuf {
    store_root.join("meta.json")
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Ensures `<store>/meta.json` exists.
///
/// Creates the file with a freshly generated ULID `store_id` if it is absent.
/// Idempotent when the file already exists — no read or update is performed.
pub fn ensure_store_meta(store_root: &Path) -> Result<()> {
    let path = meta_path(store_root);
    if path.exists() {
        return Ok(());
    }

    let meta = StoreMeta {
        store_id: Ulid::new().to_string(),
        created_at: now_iso8601(),
    };

    let json = serde_json::to_string_pretty(&meta).map_err(|e| AppError::json(path.clone(), e))?;

    // Ensure parent directory exists.
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
        {
            std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }
    }

    // Atomic write: write to a temp file then rename.
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, json).map_err(|e| AppError::io(&tmp_path, e))?;
    std::fs::rename(&tmp_path, &path).map_err(|e| AppError::io(&path, e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_meta_json_on_first_call() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path();

        ensure_store_meta(store_root).unwrap();

        let path = meta_path(store_root);
        assert!(path.exists(), "meta.json should be created");

        let contents = std::fs::read_to_string(&path).unwrap();
        let meta: StoreMeta = serde_json::from_str(&contents).unwrap();
        assert!(!meta.store_id.is_empty());
        assert!(!meta.created_at.is_empty());
    }

    #[test]
    fn idempotent_on_second_call() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path();

        ensure_store_meta(store_root).unwrap();
        let id_first = serde_json::from_str::<StoreMeta>(
            &std::fs::read_to_string(meta_path(store_root)).unwrap(),
        )
        .unwrap()
        .store_id;

        ensure_store_meta(store_root).unwrap();
        let id_second = serde_json::from_str::<StoreMeta>(
            &std::fs::read_to_string(meta_path(store_root)).unwrap(),
        )
        .unwrap()
        .store_id;

        assert_eq!(id_first, id_second, "store_id must not change on re-call");
    }
}
