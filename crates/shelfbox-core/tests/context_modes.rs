use std::path::PathBuf;

use shelfbox_core::{
    context::{self, CurrentGitContext},
    store::index::{Index, RepoEntry},
};

mod common;

#[test]
fn current_git_context_does_not_mutate_repo_tree() {
    let repo = common::init_git_repo();
    let before = common::snapshot_tree(repo.path());

    let current = context::current_git_context(repo.path()).unwrap();

    assert_eq!(current.repo_root, repo.path());
    assert_eq!(common::snapshot_tree(repo.path()), before);
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
