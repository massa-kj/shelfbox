use std::path::Path;

use crate::{
    context,
    error::{AppError, Result},
    store::{
        index::{self, Index, RepoEntry},
        scanner::{self, ScanError},
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebuildIndexWarning {
    pub repo_store_dir: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebuildIndexReport {
    pub repositories: usize,
    pub warnings: Vec<RebuildIndexWarning>,
    pub dry_run: bool,
}

/// Rebuilds `index.json` from canonical manifests under `<store>/repos/`.
///
/// Duplicate repository or item identities abort the rebuild before writing.
/// Unreadable or corrupted manifests are reported as warnings and skipped.
pub fn run(store_root: &Path, dry_run: bool) -> Result<RebuildIndexReport> {
    let scan = scanner::scan(store_root)?;
    let mut warnings = Vec::new();
    let mut hard_errors = Vec::new();

    for error in scan.errors {
        match error {
            ScanError::ReadFailed { dir, source } => warnings.push(RebuildIndexWarning {
                repo_store_dir: dir.clone(),
                message: format!("failed to read repos/{dir}/manifest.json: {source}"),
            }),
            ScanError::ParseFailed { dir, source } => warnings.push(RebuildIndexWarning {
                repo_store_dir: dir.clone(),
                message: format!("failed to parse repos/{dir}/manifest.json: {source}"),
            }),
            ScanError::DuplicateRepoId { repo_id, dirs } => hard_errors.push(format!(
                "duplicate repo_id '{repo_id}' found in {}",
                dirs.join(", ")
            )),
            ScanError::DuplicateItemId { item_id, repo_ids } => hard_errors.push(format!(
                "duplicate item_id '{item_id}' found in repo_id(s) {}",
                repo_ids.join(", ")
            )),
        }
    }

    if !hard_errors.is_empty() {
        return Err(AppError::Internal(format!(
            "cannot rebuild index: {}",
            hard_errors.join("; ")
        )));
    }

    let mut rebuilt = Index::new();
    let last_seen_at = context::now_iso8601();

    for scanned in scan.entries {
        rebuilt.upsert(
            scanned.manifest.repo_id,
            RepoEntry {
                repo_store_dir: scanned.repo_store_dir,
                root: None,
                git_dir: None,
                git_common_dir: None,
                last_seen_at: last_seen_at.clone(),
            },
        );
    }

    let repositories = rebuilt.iter().count();
    if !dry_run {
        index::save(store_root, &rebuilt)?;
    }

    Ok(RebuildIndexReport {
        repositories,
        warnings,
        dry_run,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{
        index,
        manifest::{self, Item, Manifest, OwnershipState},
    };
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
        let repo_store = store_root.join("repos").join(dir);
        manifest::save(&repo_store, manifest).unwrap();
    }

    fn write_raw_manifest(store_root: &Path, dir: &str, contents: &str) {
        let repo_store = store_root.join("repos").join(dir);
        std::fs::create_dir_all(&repo_store).unwrap();
        std::fs::write(manifest::manifest_path(&repo_store), contents).unwrap();
    }

    #[test]
    fn rebuilds_index_from_manifests_with_repo_store_dir_only() {
        let store = TempDir::new().unwrap();
        for (dir, repo_id) in [
            ("project-a", "repo-1"),
            ("project-b", "repo-2"),
            ("project-c", "repo-3"),
        ] {
            write_manifest(
                store.path(),
                dir,
                &Manifest::new(repo_id, "2026-04-29T00:00:00Z"),
            );
        }

        let report = run(store.path(), false).unwrap();

        assert_eq!(report.repositories, 3);
        assert!(report.warnings.is_empty());
        let idx = index::load(store.path()).unwrap();
        for (repo_id, dir) in [
            ("repo-1", "project-a"),
            ("repo-2", "project-b"),
            ("repo-3", "project-c"),
        ] {
            let entry = idx.get(repo_id).unwrap();
            assert_eq!(entry.repo_store_dir, dir);
            assert_eq!(entry.root, None);
            assert_eq!(entry.git_dir, None);
            assert_eq!(entry.git_common_dir, None);
        }
    }

    #[test]
    fn duplicate_repo_id_aborts_without_writing_index() {
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

        let err = run(store.path(), false).unwrap_err();

        assert!(err.to_string().contains("duplicate repo_id"));
        assert!(!index::index_path(store.path()).exists());
    }

    #[test]
    fn duplicate_item_id_aborts_without_writing_index() {
        let store = TempDir::new().unwrap();
        let mut first = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        first.add(sample_item("item-1", "repo-1", ".env"));
        let mut second = Manifest::new("repo-2", "2026-04-29T00:00:00Z");
        second.add(sample_item("item-1", "repo-2", ".env"));
        write_manifest(store.path(), "project-a", &first);
        write_manifest(store.path(), "project-b", &second);

        let err = run(store.path(), false).unwrap_err();

        assert!(err.to_string().contains("duplicate item_id"));
        assert!(!index::index_path(store.path()).exists());
    }

    #[test]
    fn corrupted_manifest_warns_and_indexes_valid_entries() {
        let store = TempDir::new().unwrap();
        write_manifest(
            store.path(),
            "good",
            &Manifest::new("repo-1", "2026-04-29T00:00:00Z"),
        );
        write_raw_manifest(store.path(), "bad", "{not json");

        let report = run(store.path(), false).unwrap();

        assert_eq!(report.repositories, 1);
        assert_eq!(report.warnings.len(), 1);
        assert_eq!(report.warnings[0].repo_store_dir, "bad");
        let idx = index::load(store.path()).unwrap();
        assert_eq!(idx.iter().count(), 1);
        assert_eq!(idx.get("repo-1").unwrap().repo_store_dir, "good");
    }

    #[test]
    fn dry_run_reports_plan_without_writing_index() {
        let store = TempDir::new().unwrap();
        write_manifest(
            store.path(),
            "project-a",
            &Manifest::new("repo-1", "2026-04-29T00:00:00Z"),
        );

        let report = run(store.path(), true).unwrap();

        assert!(report.dry_run);
        assert_eq!(report.repositories, 1);
        assert!(!index::index_path(store.path()).exists());
    }
}
