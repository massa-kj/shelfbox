use std::{collections::HashSet, path::Path};

use crate::{
    error::{AppError, Result},
    policy::gc_policy::{self, GcProtection},
    storage::operation_record_store,
    store::{
        manifest,
        scanner::{self, ScanError},
    },
};

pub use crate::plan::store_gc::{GcCandidate, GcPlan, GcReport};

pub fn plan(store_root: &Path) -> Result<GcPlan> {
    let protected_recovery_paths = operation_record_store::protected_store_paths(store_root)?;
    let scan = scanner::scan(store_root)?;
    if !scan.errors.is_empty() {
        return Err(AppError::Internal(format!(
            "store scan failed; refusing to run gc: {}",
            format_scan_errors(&scan.errors)
        )));
    }

    let mut plan = GcPlan::default();

    for repo in scan.entries {
        let repo_store = store_root.join("repos").join(&repo.repo_store_dir);

        for item in &repo.manifest.items {
            match gc_policy::classify_ownership(item.ownership_state) {
                GcProtection::Collectible => {
                    let absolute_store_path = repo_store.join(&item.store_path);
                    if protected_recovery_paths.iter().any(|protected| {
                        absolute_store_path.starts_with(protected)
                            || protected.starts_with(&absolute_store_path)
                    }) {
                        continue;
                    }
                    let (size_bytes, store_exists) = item_size(&absolute_store_path)?;
                    plan.candidates.push(GcCandidate {
                        repo_id: repo.manifest.repo_id.clone(),
                        repo_store_dir: repo.repo_store_dir.clone(),
                        item_id: item.item_id.clone(),
                        path: item.path.clone(),
                        store_path: item.store_path.clone(),
                        absolute_store_path,
                        size_bytes,
                        store_exists,
                    });
                }
                GcProtection::Attached => plan.protected_attached += 1,
                GcProtection::Detached => plan.protected_detached += 1,
                GcProtection::Unreachable => plan.protected_unreachable += 1,
            }
        }
    }

    plan.candidates.sort_by(|a, b| {
        a.repo_store_dir
            .cmp(&b.repo_store_dir)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.item_id.cmp(&b.item_id))
    });

    Ok(plan)
}

pub fn run(store_root: &Path, dry_run: bool) -> Result<GcReport> {
    let plan = plan(store_root)?;
    if dry_run {
        return Ok(GcReport {
            candidates: plan.candidates,
            dry_run: true,
            ..GcReport::default()
        });
    }

    let mut report = GcReport {
        candidates: plan.candidates.clone(),
        dry_run: false,
        ..GcReport::default()
    };

    let mut by_repo: Vec<(String, Vec<GcCandidate>)> = Vec::new();
    for candidate in plan.candidates {
        match by_repo
            .iter_mut()
            .find(|(dir, _)| dir == &candidate.repo_store_dir)
        {
            Some((_, candidates)) => candidates.push(candidate),
            None => by_repo.push((candidate.repo_store_dir.clone(), vec![candidate])),
        }
    }

    for (repo_store_dir, candidates) in by_repo {
        let repo_store = store_root.join("repos").join(&repo_store_dir);

        let mut mf = manifest::load(&repo_store)?;
        let before = mf.items.len();
        let planned_ids: HashSet<&str> = candidates
            .iter()
            .map(|candidate| candidate.item_id.as_str())
            .collect();
        let collectible_ids: HashSet<String> = mf
            .items
            .iter()
            .filter(|item| {
                gc_policy::is_collectible(item.ownership_state)
                    && planned_ids.contains(item.item_id.as_str())
            })
            .map(|item| item.item_id.clone())
            .collect();

        if collectible_ids.is_empty() {
            continue;
        }

        mf.items.retain(|item| {
            !gc_policy::is_collectible(item.ownership_state)
                || !collectible_ids.contains(&item.item_id)
        });

        if mf.items.len() != before {
            manifest::save(&repo_store, &mf)?;
            report.manifests_updated += 1;
        }

        for candidate in &candidates {
            if !collectible_ids.contains(&candidate.item_id) {
                continue;
            }
            if !candidate.store_exists {
                report.missing_items += 1;
                continue;
            }

            remove_store_path(&candidate.absolute_store_path)?;
            report.deleted_items += 1;
            report.bytes_reclaimed += candidate.size_bytes;
        }
    }

    Ok(report)
}

fn item_size(path: &Path) -> Result<(u64, bool)> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => Ok((dir_size(path)?, true)),
        Ok(meta) => Ok((meta.len(), true)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok((0, false)),
        Err(err) => Err(AppError::io(path, err)),
    }
}

fn dir_size(path: &Path) -> Result<u64> {
    let entries = std::fs::read_dir(path).map_err(|err| AppError::io(path, err))?;
    let mut total = 0;

    for entry in entries {
        let entry = entry.map_err(|err| AppError::io(path, err))?;
        let entry_path = entry.path();
        let meta = entry
            .metadata()
            .map_err(|err| AppError::io(&entry_path, err))?;
        total += if meta.is_dir() {
            dir_size(&entry_path)?
        } else {
            meta.len()
        };
    }

    Ok(total)
}

fn remove_store_path(path: &Path) -> Result<()> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(AppError::io(path, err)),
    };

    if meta.is_dir() {
        std::fs::remove_dir_all(path).map_err(|err| AppError::io(path, err))
    } else {
        std::fs::remove_file(path).map_err(|err| AppError::io(path, err))
    }
}

fn format_scan_errors(errors: &[ScanError]) -> String {
    errors
        .iter()
        .map(|error| match error {
            ScanError::ReadFailed { dir, source } => {
                format!("{dir}: read failed: {source}")
            }
            ScanError::ParseFailed { dir, source } => {
                format!("{dir}: parse failed: {source}")
            }
            ScanError::DuplicateRepoId { repo_id, dirs } => {
                format!("duplicate repo_id {repo_id} in {}", dirs.join(", "))
            }
            ScanError::DuplicateItemId { item_id, repo_ids } => {
                format!("duplicate item_id {item_id} in {}", repo_ids.join(", "))
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::store::{
        index::{self, RepoEntry},
        manifest::{Item, Manifest, OwnershipState},
    };
    use tempfile::TempDir;

    fn item(path: &str, state: OwnershipState) -> Item {
        Item {
            item_id: format!("item-{path}"),
            origin_repo_id: "repo-1".into(),
            path: path.to_string(),
            store_path: format!("items/{path}"),
            ownership_state: state,
            created_at: "2026-04-29T00:00:00Z".into(),
            updated_at: "2026-04-29T00:00:00Z".into(),
        }
    }

    fn write_repo(store: &Path, dir: &str, repo_id: &str, items: Vec<Item>) -> PathBuf {
        let repo_store = store.join("repos").join(dir);
        let mut manifest = Manifest::new(repo_id, "2026-04-29T00:00:00Z");
        for item in items {
            let store_path = repo_store.join(&item.store_path);
            if let Some(parent) = store_path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&store_path, format!("data for {}", item.path)).unwrap();
            manifest.add(item);
        }
        manifest::save(&repo_store, &manifest).unwrap();
        repo_store
    }

    fn write_index(store: &Path, repo_id: &str, repo_store_dir: &str, root: Option<PathBuf>) {
        let mut idx = index::Index::new();
        idx.upsert(
            repo_id,
            RepoEntry {
                root,
                git_dir: None,
                git_common_dir: None,
                repo_store_dir: repo_store_dir.to_string(),
                last_seen_at: "2026-04-29T00:00:00Z".into(),
            },
        );
        index::save(store, &idx).unwrap();
    }

    #[test]
    fn orphaned_items_are_deleted_and_removed_from_manifest() {
        let store = TempDir::new().unwrap();
        let repo_store = write_repo(
            store.path(),
            "project",
            "repo-1",
            vec![item("old.env", OwnershipState::Orphaned)],
        );

        let report = run(store.path(), false).unwrap();
        let manifest = manifest::load(&repo_store).unwrap();

        assert_eq!(report.deleted_items, 1);
        assert_eq!(report.manifests_updated, 1);
        assert!(!repo_store.join("items/old.env").exists());
        assert!(manifest.items.is_empty());
        assert!(repo_store.exists(), "repo store directory must remain");
    }

    #[cfg(unix)]
    #[test]
    fn manifest_save_failure_does_not_delete_orphaned_store_file() {
        use std::os::unix::fs::PermissionsExt;

        let store = TempDir::new().unwrap();
        let repo_store = write_repo(
            store.path(),
            "project",
            "repo-1",
            vec![item("old.env", OwnershipState::Orphaned)],
        );
        let store_file = repo_store.join("items/old.env");
        let original_perms = std::fs::metadata(&repo_store).unwrap().permissions();
        let mut readonly_perms = original_perms.clone();
        readonly_perms.set_mode(0o500);
        std::fs::set_permissions(&repo_store, readonly_perms).unwrap();

        let result = run(store.path(), false);

        std::fs::set_permissions(&repo_store, original_perms).unwrap();
        assert!(result.is_err());
        assert!(store_file.exists());
        let manifest = manifest::load(&repo_store).unwrap();
        assert_eq!(manifest.items.len(), 1);
        assert_eq!(manifest.items[0].ownership_state, OwnershipState::Orphaned);
    }

    #[cfg(unix)]
    #[test]
    fn delete_failure_leaves_orphaned_file_unreferenced() {
        use std::os::unix::fs::PermissionsExt;

        let store = TempDir::new().unwrap();
        let repo_store = write_repo(
            store.path(),
            "project",
            "repo-1",
            vec![item("old.env", OwnershipState::Orphaned)],
        );
        let items_dir = repo_store.join("items");
        let store_file = items_dir.join("old.env");
        let original_perms = std::fs::metadata(&items_dir).unwrap().permissions();
        let mut readonly_perms = original_perms.clone();
        readonly_perms.set_mode(0o500);
        std::fs::set_permissions(&items_dir, readonly_perms).unwrap();

        let result = run(store.path(), false);

        std::fs::set_permissions(&items_dir, original_perms).unwrap();
        assert!(result.is_err());
        assert!(store_file.exists());
        let manifest = manifest::load(&repo_store).unwrap();
        assert!(manifest.items.is_empty());
    }

    #[test]
    fn protected_states_are_skipped() {
        let store = TempDir::new().unwrap();
        let repo_store = write_repo(
            store.path(),
            "project",
            "repo-1",
            vec![
                item("attached.env", OwnershipState::Attached),
                item("detached.env", OwnershipState::Detached),
                item("unreachable.env", OwnershipState::Unreachable),
            ],
        );

        let report = run(store.path(), false).unwrap();
        let manifest = manifest::load(&repo_store).unwrap();

        assert!(report.candidates.is_empty());
        assert_eq!(report.deleted_items, 0);
        assert_eq!(manifest.items.len(), 3);
        assert!(repo_store.join("items/attached.env").exists());
        assert!(repo_store.join("items/detached.env").exists());
        assert!(repo_store.join("items/unreachable.env").exists());
    }

    #[test]
    fn unfinished_recovery_record_protects_named_store_path_from_gc() {
        use crate::{
            domain::{
                materialization::MaterializationStrategy,
                operation_record::{
                    OperationKind, OperationPhase, OperationPreState, OperationRecord,
                    RecoveryAbsolutePath, RecoveryRecord, RecoveryRecordKind,
                    OPERATION_RECORD_SCHEMA_VERSION,
                },
            },
            storage::operation_record_store,
        };

        let store = TempDir::new().unwrap();
        let repo_store = write_repo(
            store.path(),
            "project",
            "repo-1",
            vec![item("old.env", OwnershipState::Orphaned)],
        );
        let repo_root = tempfile::tempdir().unwrap();
        let record = RecoveryRecord {
            schema_version: OPERATION_RECORD_SCHEMA_VERSION,
            record_id: ulid::Ulid::new().to_string(),
            created_at: "2026-07-12T00:00:00Z".into(),
            record: RecoveryRecordKind::Operation(OperationRecord {
                operation: OperationKind::Move,
                phase: OperationPhase::StoreTransferred,
                repo_id: "repo-1".into(),
                repo_root: RecoveryAbsolutePath::new(repo_root.path()).unwrap(),
                repo_store_path: None,
                strategy: MaterializationStrategy::Copy,
                direction: None,
                pre_state: OperationPreState {
                    store_path: Some("repos/project/items/old.env".parse().unwrap()),
                    ..OperationPreState::default()
                },
                post_state: None,
                artifact_record_ids: Vec::new(),
                backup: None,
            }),
        };
        operation_record_store::create(store.path(), &record).unwrap();

        let plan = plan(store.path()).unwrap();
        assert!(plan.candidates.is_empty());
        assert!(repo_store.join("items/old.env").exists());
    }

    #[test]
    fn unreachable_repo_store_is_not_deleted() {
        let store = TempDir::new().unwrap();
        let missing_root = store.path().join("missing-clone");
        let repo_store = write_repo(
            store.path(),
            "project",
            "repo-1",
            vec![item("still-owned.env", OwnershipState::Unreachable)],
        );
        write_index(store.path(), "repo-1", "project", Some(missing_root));

        let report = run(store.path(), false).unwrap();

        assert!(report.candidates.is_empty());
        assert!(repo_store.exists());
        assert!(repo_store.join("items/still-owned.env").exists());
    }

    #[test]
    fn root_none_does_not_make_attached_items_collectable() {
        let store = TempDir::new().unwrap();
        let repo_store = write_repo(
            store.path(),
            "project",
            "repo-1",
            vec![item("attached.env", OwnershipState::Attached)],
        );
        write_index(store.path(), "repo-1", "project", None);

        let report = run(store.path(), false).unwrap();

        assert!(report.candidates.is_empty());
        assert!(repo_store.join("items/attached.env").exists());
    }

    #[test]
    fn missing_root_does_not_make_non_orphaned_items_collectable() {
        let store = TempDir::new().unwrap();
        let repo_store = write_repo(
            store.path(),
            "project",
            "repo-1",
            vec![item("detached.env", OwnershipState::Detached)],
        );
        write_index(
            store.path(),
            "repo-1",
            "project",
            Some(store.path().join("missing-root")),
        );

        let report = run(store.path(), false).unwrap();

        assert!(report.candidates.is_empty());
        assert!(repo_store.join("items/detached.env").exists());
    }

    #[test]
    fn dry_run_reports_without_writing() {
        let store = TempDir::new().unwrap();
        let repo_store = write_repo(
            store.path(),
            "project",
            "repo-1",
            vec![item("old.env", OwnershipState::Orphaned)],
        );
        let manifest_before =
            std::fs::read_to_string(manifest::manifest_path(&repo_store)).unwrap();

        let report = run(store.path(), true).unwrap();

        assert!(report.dry_run);
        assert_eq!(report.candidates.len(), 1);
        assert_eq!(report.deleted_items, 0);
        assert!(repo_store.join("items/old.env").exists());
        assert_eq!(
            std::fs::read_to_string(manifest::manifest_path(&repo_store)).unwrap(),
            manifest_before
        );
    }

    #[test]
    fn empty_gc_set_reports_no_candidates() {
        let store = TempDir::new().unwrap();
        write_repo(
            store.path(),
            "project",
            "repo-1",
            vec![item("attached.env", OwnershipState::Attached)],
        );

        let plan = plan(store.path()).unwrap();

        assert!(plan.candidates.is_empty());
        assert_eq!(plan.protected_attached, 1);
    }
}
