use std::path::{Path, PathBuf};

use crate::{
    error::{AppError, Result},
    storage::{
        atomic_write::{self, ParentDirMode},
        layout,
    },
};

pub use crate::domain::index::{Index, RepoEntry};

pub fn index_path(store_root: &Path) -> PathBuf {
    layout::index_path(store_root)
}

pub fn load(store_root: &Path) -> Result<Index> {
    let path = index_path(store_root);
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| AppError::json(path, e)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Index::new()),
        Err(e) => Err(AppError::io(path, e)),
    }
}

pub fn save(store_root: &Path, index: &Index) -> Result<()> {
    let path = index_path(store_root);
    let json = serde_json::to_string_pretty(index).map_err(|e| AppError::json(path.clone(), e))?;

    atomic_write::write(&path, &json, ParentDirMode::Private)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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
    fn round_trip_empty_index() {
        let dir = TempDir::new().unwrap();
        let index = Index::new();
        save(dir.path(), &index).unwrap();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.len(), 0);
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
        assert_eq!(entry.root.as_deref(), Some(Path::new("/home/user/myapp")));
    }

    #[test]
    fn save_uses_atomic_temp_path_and_cleans_it_up() {
        let dir = TempDir::new().unwrap();
        save(dir.path(), &Index::new()).unwrap();

        assert!(index_path(dir.path()).exists());
        assert!(!dir.path().join("index.json.tmp").exists());
    }

    #[test]
    fn missing_file_returns_empty_index() {
        let dir = TempDir::new().unwrap();
        let index = load(dir.path()).unwrap();
        assert!(index.is_empty());
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
        assert_eq!(
            loaded.get(id).unwrap().root.as_deref(),
            Some(Path::new("/new/path"))
        );
    }

    #[test]
    fn round_trip_with_missing_git_fields() {
        let json = r#"{
            "version": 1,
            "repos": {
                "01JWPQ3VKGE93V9BDHAENVXFA5": {
                    "repo_store_dir": "myapp",
                    "last_seen_at": "2026-04-29T00:00:00Z"
                }
            }
        }"#;

        let index: Index = serde_json::from_str(json).unwrap();
        let entry = index.get("01JWPQ3VKGE93V9BDHAENVXFA5").unwrap();
        assert_eq!(entry.repo_store_dir, "myapp");
        assert_eq!(entry.root, None);
        assert_eq!(entry.git_dir, None);
        assert_eq!(entry.git_common_dir, None);

        let serialized = serde_json::to_string(&index).unwrap();
        let loaded: Index = serde_json::from_str(&serialized).unwrap();
        let loaded_entry = loaded.get("01JWPQ3VKGE93V9BDHAENVXFA5").unwrap();
        assert_eq!(loaded_entry.root, None);
        assert_eq!(loaded_entry.git_dir, None);
        assert_eq!(loaded_entry.git_common_dir, None);
    }

    #[test]
    fn find_by_root_skips_entries_without_root() {
        let mut index = Index::new();
        index.upsert(
            "01JWPQ3VKGE93V9BDHAENVXFA5",
            RepoEntry {
                root: None,
                git_dir: None,
                git_common_dir: None,
                repo_store_dir: "myapp".into(),
                last_seen_at: "2026-04-29T00:00:00Z".into(),
            },
        );

        assert_eq!(index.find_by_root(Path::new("/home/user/myapp")), None);
    }
}
