//! Compile-time fixture for the public v0.8.0 API inherited by v0.8.1.
//!
//! This test intentionally uses struct literals and exact function-pointer
//! signatures. An incompatible field or signature change must therefore be
//! reviewed explicitly instead of slipping into the copy-mode implementation.

use std::path::{Path, PathBuf};

use shelfbox_core::{
    api::{config, item, repo, store},
    config::Config,
    error::{AppError, Result},
};

#[test]
fn directionless_operation_signatures_remain_source_compatible() {
    let _: fn(&Path, Option<&Path>) -> Result<item::RepoContext> = item::build_create_or_load;
    let _: fn(&Path, Option<&Path>) -> Result<item::RepoContext> =
        item::build_preview_create_or_load;
    let _: fn(&Path, Option<&Path>) -> Result<item::ReadOnlyRepoContext> = item::build_read_only;
    let _: fn(&mut item::RepoContext, &Path, bool) -> Result<item::ItemAddReport> = item::add_file;
    let _: fn(&mut item::RepoContext, &Path, bool) -> Result<item::DirectoryAddResult> =
        item::add_directory;
    let _: fn(&mut item::RepoContext, &Path, bool, bool, bool) -> Result<item::ItemRestoreReport> =
        item::restore_file;
    let _: fn(
        &mut item::RepoContext,
        &str,
        bool,
        bool,
        bool,
    ) -> Result<item::NamespaceRestoreResult> = item::restore_namespace;
    let _: for<'a> fn(&'a item::RepoContext) -> &'a [item::Item] = item::list;
    let _: fn(&item::RepoContext) -> Result<Vec<item::ItemStatus>> = item::status;
    let _: fn(&item::RepoContext, &Path, bool, bool) -> Result<item::ItemRepairReport> =
        item::repair;
    let _: fn(&mut item::RepoContext, &Path, bool) -> Result<item::ItemRelinkReport> = item::relink;
    let _: fn(&mut item::RepoContext, &Path, &Path, bool) -> Result<item::ItemMoveReport> =
        item::move_item;
    let _: fn(&item::RepoContext, &Path) -> Result<item::ItemInfo> = item::info;
    let _: fn(&item::ReadOnlyRepoContext, &Path) -> Result<item::ItemInfo> = item::info_read_only;

    let _: fn(&repo::RepoContext) -> Result<repo::IntegrityReport> = repo::integrity_check;
    let _: fn(&repo::RepoContext, &Config) -> Result<repo::TransitionReport> =
        repo::scan_transitions;
    let _: fn(&mut repo::RepoContext, bool, bool) -> Result<repo::RepairRepoReport> =
        repo::repair_repo;
    let _: fn(Option<&repo::Manifest>) -> Result<()> = repo::check_reclaim_precondition;
    let _: fn(&repo::ExplicitReclaimContext) -> Result<repo::ReclaimPlan> = repo::plan_reclaim;
    let _: fn(&repo::ExplicitReclaimContext) -> Result<repo::ReclaimOutcome> =
        repo::execute_reclaim;
}

#[test]
fn status_report_and_plan_fields_remain_source_compatible() {
    let status = item::ItemStatus {
        path: "secret.txt".into(),
        link_exists: true,
        link_valid: true,
        store_exists: true,
        in_exclude: true,
        not_tracked: true,
        ok: true,
    };
    let _integrity = repo::IntegrityReport {
        items: vec![status],
        orphan_store_items: Vec::new(),
        repo_root_matches_index: true,
    };
    let _info = item::ItemInfo {
        path: "secret.txt".into(),
        repo_root: PathBuf::from("repo"),
        store_path: Some(PathBuf::from("store/items/secret.txt")),
        link_target: Some(PathBuf::from("store/items/secret.txt")),
        symlink_ok: true,
        tracked: true,
        in_exclude: true,
    };

    let add_plan = item::ItemAddPlan {
        path: "secret.txt".into(),
        abs_path: PathBuf::from("repo/secret.txt"),
        store_path: PathBuf::from("store/items/secret.txt"),
        store_path_relative: "items/secret.txt".into(),
    };
    let _add_report = item::ItemAddReport {
        plan: add_plan,
        dry_run: true,
    };

    let restore_plan = item::ItemRestorePlan {
        path: "secret.txt".into(),
        abs_path: PathBuf::from("repo/secret.txt"),
        store_path: PathBuf::from("store/items/secret.txt"),
        keep_ignore: false,
        action: item::ItemRestoreAction::RestoreFile,
    };
    let _restore_report = item::ItemRestoreReport {
        plan: restore_plan,
        dry_run: true,
    };

    let relink_plan = item::ItemRelinkPlan {
        path: "secret.txt".into(),
        abs_path: PathBuf::from("repo/secret.txt"),
        store_path: PathBuf::from("store/items/secret.txt"),
        symlink_ok: false,
    };
    let _relink_report = item::ItemRelinkReport {
        plan: relink_plan,
        outcome: item::RelinkOutcome::WouldRelink,
        dry_run: true,
    };

    let move_plan = item::ItemMovePlan {
        old_path: "old.txt".into(),
        new_path: "new.txt".into(),
        old_abs_path: PathBuf::from("repo/old.txt"),
        new_abs_path: PathBuf::from("repo/new.txt"),
        old_store_path: PathBuf::from("store/items/old.txt"),
        new_store_path: PathBuf::from("store/items/new.txt"),
        old_store_path_relative: "items/old.txt".into(),
        new_store_path_relative: "items/new.txt".into(),
    };
    let _move_report = item::ItemMoveReport {
        plan: move_plan,
        dry_run: true,
        warnings: vec![item::ItemMoveWarning::ExcludeRemoveFailed {
            path: "old.txt".into(),
            message: "fixture".into(),
        }],
    };

    let repair_action = item::RepoRepairSymlinkAction::Recreate {
        path: "secret.txt".into(),
        abs_path: PathBuf::from("repo/secret.txt"),
        store_path: PathBuf::from("store/items/secret.txt"),
    };
    let _item_repair = item::ItemRepairReport {
        action: repair_action.clone(),
        outcome: item::RepairOutcome::LinkRecreated,
        dry_run: true,
    };
    let repo_repair_plan = repo::RepoRepairPlan {
        symlink_actions: vec![repair_action],
        exclude_paths: vec!["secret.txt".into()],
        exclude_updated: true,
        index_updated: false,
        hints_updated: false,
    };
    let _repo_repair = repo::RepairRepoReport {
        plan: repo_repair_plan,
        symlinks_repaired: 1,
        symlinks_already_healthy: 0,
        symlinks_failed: Vec::new(),
        exclude_updated: true,
        index_updated: false,
        hints_updated: false,
    };
}

#[test]
fn store_reports_and_error_variants_remain_source_compatible() {
    let candidate = store::GcCandidate {
        repo_id: "repo-id".into(),
        repo_store_dir: "repo".into(),
        item_id: "item-id".into(),
        path: "secret.txt".into(),
        store_path: "items/secret.txt".into(),
        absolute_store_path: PathBuf::from("store/repo/items/secret.txt"),
        size_bytes: 6,
        store_exists: true,
    };
    let _gc_plan = store::GcPlan {
        candidates: vec![candidate.clone()],
        protected_attached: 1,
        protected_detached: 0,
        protected_unreachable: 0,
    };
    let _gc_report = store::GcReport {
        candidates: vec![candidate],
        deleted_items: 0,
        missing_items: 0,
        bytes_reclaimed: 0,
        manifests_updated: 0,
        dry_run: true,
    };
    let _rebuild = store::RebuildIndexReport {
        repositories: 1,
        warnings: vec![store::RebuildIndexWarning {
            repo_store_dir: "repo".into(),
            message: "fixture".into(),
        }],
        dry_run: true,
    };

    let mismatch = AppError::RepairSymlinkTargetMismatch {
        path: PathBuf::from("repo/secret.txt"),
        actual_target: PathBuf::from("wrong"),
        expected_target: PathBuf::from("store/items/secret.txt"),
    };
    match mismatch {
        AppError::RepairSymlinkTargetMismatch {
            path,
            actual_target,
            expected_target,
        } => {
            let _: (PathBuf, PathBuf, PathBuf) = (path, actual_target, expected_target);
        }
        _ => unreachable!(),
    }

    let regular = AppError::PathIsRegularFile {
        path: PathBuf::from("repo/secret.txt"),
    };
    assert!(matches!(regular, AppError::PathIsRegularFile { .. }));
    let _: Result<()> = Err(AppError::NotManagedLink {
        path: PathBuf::from("repo/secret.txt"),
    });
}

#[test]
fn config_and_store_function_signatures_remain_source_compatible() {
    let _: fn(Option<&Path>) -> Result<config::Config> = config::load;
    let _: fn(Option<&Path>) -> Result<config::ResolvedConfig> = config::load_resolved;
    let _: fn() -> Option<PathBuf> = config::config_file_path;
    let _: fn(&str, &str) -> Result<()> = config::set_key;

    let _: fn(&Path) -> Result<store::GcPlan> = store::gc_plan;
    let _: fn(&Path, bool) -> Result<store::GcReport> = store::gc_run;
    let _: fn(&Path, bool) -> Result<store::RebuildIndexReport> = store::rebuild_index;
    let _: fn(&Path, bool) -> Result<store::MigrationReport> = store::migrate_manifests;
}
