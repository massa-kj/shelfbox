use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
};

use crate::{
    error::{AppError, Result},
    storage::{layout, manifest_store},
};

pub use crate::storage::manifest_store::Manifest;

#[derive(Debug, Default)]
pub struct ScanResult {
    pub entries: Vec<ScannedRepo>,
    pub errors: Vec<ScanError>,
}

#[derive(Debug)]
pub struct ScannedRepo {
    pub repo_store_dir: String,
    pub manifest: Manifest,
}

#[derive(Debug)]
pub enum ScanError {
    ReadFailed {
        dir: String,
        source: io::Error,
    },
    ParseFailed {
        dir: String,
        source: serde_json::Error,
    },
    DuplicateRepoId {
        repo_id: String,
        dirs: Vec<String>,
    },
    DuplicateItemId {
        item_id: String,
        repo_ids: Vec<String>,
    },
}

pub fn scan(store_root: &Path) -> Result<ScanResult> {
    let repos_dir = layout::repos_dir(store_root);
    let entries = match std::fs::read_dir(&repos_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(ScanResult::default()),
        Err(e) => return Err(AppError::io(repos_dir, e)),
    };

    let mut repo_dirs = Vec::new();
    let mut result = ScanResult::default();

    for entry in entries {
        match entry {
            Ok(entry) => {
                let path = entry.path();
                if path.is_dir() {
                    repo_dirs.push(path);
                }
            }
            Err(source) => result.errors.push(ScanError::ReadFailed {
                dir: repos_dir.display().to_string(),
                source,
            }),
        }
    }

    repo_dirs.sort();

    for repo_store in repo_dirs {
        let repo_store_dir = repo_store
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| repo_store.display().to_string());
        let manifest_path = manifest_store::manifest_path(&repo_store);

        let contents = match std::fs::read_to_string(&manifest_path) {
            Ok(contents) => contents,
            Err(source) => {
                result.errors.push(ScanError::ReadFailed {
                    dir: repo_store_dir,
                    source,
                });
                continue;
            }
        };

        match serde_json::from_str::<Manifest>(&contents) {
            Ok(manifest) => result.entries.push(ScannedRepo {
                repo_store_dir,
                manifest,
            }),
            Err(source) => result.errors.push(ScanError::ParseFailed {
                dir: repo_store_dir,
                source,
            }),
        }
    }

    detect_duplicates(&mut result);
    Ok(result)
}

fn detect_duplicates(result: &mut ScanResult) {
    let mut repo_dirs_by_id: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut item_repos_by_id: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for entry in &result.entries {
        repo_dirs_by_id
            .entry(entry.manifest.repo_id.clone())
            .or_default()
            .push(entry.repo_store_dir.clone());

        for item in &entry.manifest.items {
            item_repos_by_id
                .entry(item.item_id.clone())
                .or_default()
                .insert(entry.manifest.repo_id.clone());
        }
    }

    for (repo_id, dirs) in repo_dirs_by_id {
        if dirs.len() > 1 {
            result
                .errors
                .push(ScanError::DuplicateRepoId { repo_id, dirs });
        }
    }

    for (item_id, repo_ids) in item_repos_by_id {
        if repo_ids.len() > 1 {
            result.errors.push(ScanError::DuplicateItemId {
                item_id,
                repo_ids: repo_ids.into_iter().collect(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::manifest_store::{Item, OwnershipState};
    use tempfile::TempDir;

    fn sample_item(item_id: &str, repo_id: &str, path: &str) -> Item {
        Item {
            item_id: item_id.to_string(),
            origin_repo_id: repo_id.to_string(),
            path: path.to_string(),
            store_path: format!("items/{path}"),
            ownership_state: OwnershipState::Attached,
            created_at: "2026-04-29T00:00:00Z".into(),
            updated_at: "2026-04-29T00:00:00Z".into(),
        }
    }

    fn write_manifest(store_root: &Path, dir: &str, manifest: &Manifest) {
        let repo_store = layout::repo_store_path(store_root, dir);
        manifest_store::save(&repo_store, manifest).unwrap();
    }

    fn write_raw_manifest(store_root: &Path, dir: &str, contents: &str) {
        let repo_store = layout::repo_store_path(store_root, dir);
        std::fs::create_dir_all(&repo_store).unwrap();
        std::fs::write(manifest_store::manifest_path(&repo_store), contents).unwrap();
    }

    #[test]
    fn empty_repos_returns_empty_result() {
        let store = TempDir::new().unwrap();

        let result = scan(store.path()).unwrap();

        assert!(result.entries.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn one_valid_manifest_is_returned() {
        let store = TempDir::new().unwrap();
        let manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        write_manifest(store.path(), "my-project", &manifest);

        let result = scan(store.path()).unwrap();

        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].repo_store_dir, "my-project");
        assert_eq!(result.entries[0].manifest.repo_id, "repo-1");
        assert!(result.errors.is_empty());
    }

    #[test]
    fn corrupted_json_is_recorded_as_parse_failed() {
        let store = TempDir::new().unwrap();
        write_raw_manifest(store.path(), "bad", "{not json");

        let result = scan(store.path()).unwrap();

        assert!(result.entries.is_empty());
        assert!(matches!(
            result.errors.as_slice(),
            [ScanError::ParseFailed { dir, .. }] if dir == "bad"
        ));
    }

    #[test]
    fn duplicate_repo_ids_are_reported() {
        let store = TempDir::new().unwrap();
        write_manifest(
            store.path(),
            "project-a",
            &Manifest::new("repo-1", "2026-04-29T00:00:00Z"),
        );
        write_manifest(
            store.path(),
            "project-b",
            &Manifest::new("repo-1", "2026-04-29T00:00:00Z"),
        );

        let result = scan(store.path()).unwrap();

        assert_eq!(result.entries.len(), 2);
        assert!(matches!(
            result.errors.as_slice(),
            [ScanError::DuplicateRepoId { repo_id, dirs }]
                if repo_id == "repo-1"
                    && dirs == &vec!["project-a".to_string(), "project-b".to_string()]
        ));
    }

    #[test]
    fn duplicate_item_ids_are_reported() {
        let store = TempDir::new().unwrap();
        let mut first = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        first.add(sample_item("item-1", "repo-1", ".env"));
        let mut second = Manifest::new("repo-2", "2026-04-29T00:00:00Z");
        second.add(sample_item("item-1", "repo-2", ".env"));
        write_manifest(store.path(), "project-a", &first);
        write_manifest(store.path(), "project-b", &second);

        let result = scan(store.path()).unwrap();

        assert_eq!(result.entries.len(), 2);
        assert!(matches!(
            result.errors.as_slice(),
            [ScanError::DuplicateItemId { item_id, repo_ids }]
                if item_id == "item-1"
                    && repo_ids == &vec!["repo-1".to_string(), "repo-2".to_string()]
        ));
    }

    #[test]
    fn valid_entries_are_returned_alongside_errors() {
        let store = TempDir::new().unwrap();
        write_manifest(
            store.path(),
            "good",
            &Manifest::new("repo-1", "2026-04-29T00:00:00Z"),
        );
        write_raw_manifest(store.path(), "bad", "{not json");

        let result = scan(store.path()).unwrap();

        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].repo_store_dir, "good");
        assert!(matches!(
            result.errors.as_slice(),
            [ScanError::ParseFailed { dir, .. }] if dir == "bad"
        ));
    }

    #[test]
    fn scan_does_not_rewrite_manifest_files() {
        let store = TempDir::new().unwrap();
        let manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        write_manifest(store.path(), "project-a", &manifest);
        let manifest_path =
            manifest_store::manifest_path(&layout::repo_store_path(store.path(), "project-a"));
        let before = std::fs::read_to_string(&manifest_path).unwrap();

        let result = scan(store.path()).unwrap();

        assert_eq!(result.entries.len(), 1);
        assert_eq!(std::fs::read_to_string(&manifest_path).unwrap(), before);
    }
}
