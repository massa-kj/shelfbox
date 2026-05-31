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
/// Provides a stable identity for the store directory, useful for diagnosing
/// store relocations and generating human-readable context in error messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreMeta {
    /// Globally unique ULID assigned when the store is first created.
    pub store_id: String,

    /// ISO-8601 timestamp when this store was initialised.
    pub created_at: String,

    /// Hostname of the machine that created this store.
    ///
    /// Recorded for provenance display only.  This field MUST NOT be used as
    /// an identity source, index key, or in any ownership or lookup logic.
    /// Hostnames are mutable (rename), ambiguous (container, WSL, CI, machine
    /// copy), and unstable across environments.  An empty string means the
    /// hostname could not be determined at creation time.
    #[serde(default)]
    pub hostname: String,
}

// ── Path helper ───────────────────────────────────────────────────────────────

fn meta_path(store_root: &Path) -> PathBuf {
    store_root.join("meta.json")
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Returns the machine hostname for provenance recording only.
///
/// Falls back to an empty string on failure.  The return value MUST NOT be
/// used as an identity source or lookup key — see `StoreMeta::hostname`.
fn get_hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
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
        hostname: get_hostname(),
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

/// Loads `<store>/meta.json`.
///
/// Returns `Ok(None)` if the file does not yet exist.
pub fn load_store_meta(store_root: &Path) -> Result<Option<StoreMeta>> {
    let path = meta_path(store_root);
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))?;
    let meta = serde_json::from_str(&contents).map_err(|e| AppError::json(path, e))?;
    Ok(Some(meta))
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
        // hostname may be empty in sandboxed CI; just check the field is present
        let _ = meta.hostname;
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

    #[test]
    fn load_store_meta_returns_none_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let result = load_store_meta(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn load_store_meta_roundtrips_after_ensure() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path();

        ensure_store_meta(store_root).unwrap();
        let meta = load_store_meta(store_root).unwrap().unwrap();

        assert!(!meta.store_id.is_empty());
        assert!(!meta.created_at.is_empty());
    }

    #[test]
    fn deserialises_legacy_meta_without_hostname() {
        // Old meta.json files (written before P6) have no hostname field.
        // #[serde(default)] must produce an empty string, not an error.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.json");
        std::fs::write(
            &path,
            r#"{"store_id":"01JTARXXXXXXXXXXXXXXXX","created_at":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();

        let meta = load_store_meta(dir.path()).unwrap().unwrap();
        assert_eq!(meta.store_id, "01JTARXXXXXXXXXXXXXXXX");
        assert_eq!(meta.hostname, "");
    }
}
