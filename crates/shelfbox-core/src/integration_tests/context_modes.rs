use std::path::PathBuf;

use shelfbox_core::{
    api,
    context::{self, CurrentGitContext},
    domain::{
        materialization::MaterializationStrategy,
        operation_record::{
            OperationKind, OperationPhase, OperationPreState, OperationRecord,
            RecoveryAbsolutePath, RecoveryRecord, RecoveryRecordKind,
            OPERATION_RECORD_SCHEMA_VERSION,
        },
    },
    error::AppError,
    storage::operation_record_store,
    store::{
        index::{self, Index, RepoEntry},
        manifest,
    },
};
use tempfile::TempDir;

use crate::integration_test_common as common;

#[test]
fn current_git_context_does_not_mutate_repo_tree() {
    let repo = common::init_git_repo();
    let before = common::snapshot_tree(repo.path());

    let current = context::current_git_context(repo.path()).unwrap();

    common::assert_same_path(&current.repo_root, repo.path());
    assert_eq!(common::snapshot_tree(repo.path()), before);
}

#[test]
fn read_only_context_for_unassociated_repo_does_not_initialize_store() {
    let repo = common::init_git_repo();
    let store = TempDir::new().unwrap();
    let before = common::snapshot_tree(store.path());

    let read_only = context::build_read_only(repo.path(), Some(store.path())).unwrap();

    common::assert_same_path(&read_only.current.repo_root, repo.path());
    assert_eq!(read_only.config.store, store.path());
    assert!(read_only.repo.is_none());
    assert_eq!(common::snapshot_tree(store.path()), before);
    assert_absent(store.path(), "meta.json");
    assert_absent(store.path(), "index.json");
    assert_absent(store.path(), "repos");
    assert_absent(store.path(), ".lock");
}

#[test]
fn read_only_context_loads_associated_repo_without_writing() {
    let repo = common::init_git_repo();
    let store = TempDir::new().unwrap();

    {
        let ctx = context::build_create_or_load(repo.path(), Some(store.path())).unwrap();
        manifest::save(&ctx.repo_store, &ctx.manifest).unwrap();
    }

    let before = common::snapshot_tree(store.path());
    let index_before = std::fs::read_to_string(index::index_path(store.path())).unwrap();

    let read_only = context::build_read_only(repo.path(), Some(store.path())).unwrap();

    let ctx = read_only.repo.expect("associated repo should resolve");
    common::assert_same_path(&ctx.repo_root, repo.path());
    assert_eq!(ctx.config.store, store.path());
    assert_eq!(common::snapshot_tree(store.path()), before);
    assert_eq!(
        std::fs::read_to_string(index::index_path(store.path())).unwrap(),
        index_before,
        "read-only context must not update last_seen_at"
    );
}

#[test]
fn mutating_context_blocks_on_unfinished_record_before_creating_a_repo_identity() {
    let repo = common::init_git_repo();
    let store = TempDir::new().unwrap();
    let record = unfinished_record(repo.path());
    operation_record_store::create(store.path(), &record).unwrap();

    let error = context::build_create_or_load(repo.path(), Some(store.path())).unwrap_err();

    assert!(matches!(error, AppError::RecoveryBlocked { .. }));
    assert!(!index::index_path(store.path()).exists());
    assert_eq!(
        operation_record_store::load_all(store.path()).unwrap(),
        vec![record],
        "the blocker must remain for deterministic later recovery"
    );
}

#[test]
fn read_only_context_does_not_clean_unfinished_records() {
    let repo = common::init_git_repo();
    let store = TempDir::new().unwrap();

    {
        let ctx = context::build_create_or_load(repo.path(), Some(store.path())).unwrap();
        manifest::save(&ctx.repo_store, &ctx.manifest).unwrap();
    }
    let record = unfinished_record(repo.path());
    operation_record_store::create(store.path(), &record).unwrap();
    let before = common::snapshot_tree(store.path());

    let _ = context::build_read_only(repo.path(), Some(store.path())).unwrap();

    assert_eq!(common::snapshot_tree(store.path()), before);
    assert_eq!(
        operation_record_store::load_all(store.path()).unwrap(),
        vec![record]
    );
}

#[test]
fn v2_status_reports_unfinished_recovery_without_mutating_it() {
    let repo = common::init_git_repo();
    let store = TempDir::new().unwrap();
    let item_path = repo.path().join("secret.env");
    std::fs::write(&item_path, "secret").unwrap();
    let mut ctx = context::build_create_or_load(repo.path(), Some(store.path())).unwrap();
    api::item::add_file(&mut ctx, &item_path, false).unwrap();
    let record = unfinished_record(repo.path());
    operation_record_store::create(store.path(), &record).unwrap();

    let statuses = api::item::status_v2(&ctx, api::item::StatusOptions::v2()).unwrap();

    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].severity, api::item::StatusSeverity::Error);
    assert!(statuses[0]
        .issues
        .iter()
        .any(|issue| { issue.code == api::item::StatusIssueCode::UnfinishedOperationConflict }));
    assert_eq!(
        operation_record_store::load_all(store.path()).unwrap(),
        vec![record]
    );
}

#[test]
fn resolve_existing_repo_prefers_root_match() {
    let current = CurrentGitContext {
        repo_root: PathBuf::from("/work/current"),
        git_dir: PathBuf::from("/work/current/.git"),
        git_common_dir: PathBuf::from("/shared/git"),
        remote_hint: None,
    };
    let mut index = Index::new();
    index.upsert(
        "repo-from-common-dir",
        entry(None, Some("/shared/git"), "common-dir-store"),
    );
    index.upsert(
        "repo-from-root",
        entry(Some("/work/current"), Some("/other/git"), "root-store"),
    );

    let resolved = context::resolve_existing_repo(&current, &index);

    assert_eq!(resolved.as_deref(), Some("repo-from-root"));
}

#[test]
fn resolve_existing_repo_falls_back_to_git_common_dir() {
    let current = CurrentGitContext {
        repo_root: PathBuf::from("/work/current"),
        git_dir: PathBuf::from("/work/current/.git"),
        git_common_dir: PathBuf::from("/shared/git"),
        remote_hint: None,
    };
    let mut index = Index::new();
    index.upsert(
        "repo-from-common-dir",
        entry(Some("/old/root"), Some("/shared/git"), "common-dir-store"),
    );

    let resolved = context::resolve_existing_repo(&current, &index);

    assert_eq!(resolved.as_deref(), Some("repo-from-common-dir"));
}

#[test]
fn resolve_existing_repo_returns_none_for_unassociated_repo() {
    let current = CurrentGitContext {
        repo_root: PathBuf::from("/work/current"),
        git_dir: PathBuf::from("/work/current/.git"),
        git_common_dir: PathBuf::from("/shared/git"),
        remote_hint: None,
    };
    let mut index = Index::new();
    index.upsert(
        "other-repo",
        entry(Some("/work/other"), Some("/work/other/.git"), "other-store"),
    );

    let resolved = context::resolve_existing_repo(&current, &index);

    assert_eq!(resolved, None);
}

fn entry(root: Option<&str>, git_common_dir: Option<&str>, repo_store_dir: &str) -> RepoEntry {
    RepoEntry {
        root: root.map(PathBuf::from),
        git_dir: root.map(|root| PathBuf::from(root).join(".git")),
        git_common_dir: git_common_dir.map(PathBuf::from),
        repo_store_dir: repo_store_dir.to_string(),
        last_seen_at: "2026-04-29T00:00:00Z".into(),
    }
}

fn assert_absent(root: &std::path::Path, rel: &str) {
    assert!(
        !root.join(rel).exists(),
        "expected {} to remain absent",
        root.join(rel).display()
    );
}

fn unfinished_record(repo_root: &std::path::Path) -> RecoveryRecord {
    RecoveryRecord {
        schema_version: OPERATION_RECORD_SCHEMA_VERSION,
        durability: crate::domain::mutation_durability::MutationDurability::Require,
        record_id: ulid::Ulid::new().to_string(),
        created_at: "2026-07-12T00:00:00Z".into(),
        record: RecoveryRecordKind::Operation(OperationRecord {
            operation: OperationKind::Add,
            phase: OperationPhase::RecordCreated,
            repo_id: "repo-1".into(),
            repo_root: RecoveryAbsolutePath::new(repo_root).unwrap(),
            repo_store_path: None,
            strategy: MaterializationStrategy::Copy,
            direction: None,
            pre_state: OperationPreState {
                repo_path: Some("secret.env".parse().unwrap()),
                ..OperationPreState::default()
            },
            post_state: None,
            artifact_record_ids: Vec::new(),
            backup: None,
        }),
    }
}
