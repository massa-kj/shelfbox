use std::path::Path;

use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::{
    context::now_iso8601,
    error::{AppError, Result},
    storage::{
        atomic_write::{self, ParentDirMode},
        layout,
    },
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreMeta {
    pub store_id: String,
    pub created_at: String,
    #[serde(default)]
    pub hostname: String,
}

fn get_hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

pub fn ensure_store_meta(store_root: &Path) -> Result<()> {
    let path = layout::meta_path(store_root);
    if path.exists() {
        return Ok(());
    }

    let meta = StoreMeta {
        store_id: Ulid::new().to_string(),
        created_at: now_iso8601(),
        hostname: get_hostname(),
    };

    let json = serde_json::to_string_pretty(&meta).map_err(|e| AppError::json(path.clone(), e))?;
    atomic_write::write(&path, json, ParentDirMode::Private)
}

pub fn load_store_meta(store_root: &Path) -> Result<Option<StoreMeta>> {
    let path = layout::meta_path(store_root);
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

        let path = layout::meta_path(store_root);
        assert!(path.exists(), "meta.json should be created");

        let contents = std::fs::read_to_string(&path).unwrap();
        let meta: StoreMeta = serde_json::from_str(&contents).unwrap();
        assert!(!meta.store_id.is_empty());
        assert!(!meta.created_at.is_empty());
        let _ = meta.hostname;
    }

    #[test]
    fn idempotent_on_second_call() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path();

        ensure_store_meta(store_root).unwrap();
        let id_first = serde_json::from_str::<StoreMeta>(
            &std::fs::read_to_string(layout::meta_path(store_root)).unwrap(),
        )
        .unwrap()
        .store_id;

        ensure_store_meta(store_root).unwrap();
        let id_second = serde_json::from_str::<StoreMeta>(
            &std::fs::read_to_string(layout::meta_path(store_root)).unwrap(),
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
        let dir = tempfile::tempdir().unwrap();
        let path = layout::meta_path(dir.path());
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
