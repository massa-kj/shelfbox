use std::path::{Path, PathBuf};

use tempfile::TempDir;

use shelfbox_core::{
    context,
    ignore::{GitInfoExclude, IgnoreBackend},
    link::{DefaultLinkStrategy, LinkStrategy},
    ops,
    store::{
        index,
        manifest::{self, Item, Manifest, OwnershipState},
    },
};

mod common;

fn require_symlink_support() -> bool {
    common::require_symlink_support()
}

fn linked_worktree(branch: &str, name: &str) -> (TempDir, TempDir, PathBuf) {
    let main_repo = common::init_git_repo_with_commit();
    let worktree_base = TempDir::new().unwrap();
    let worktree_path = worktree_base.path().join(name);
    let worktree_arg = worktree_path.to_string_lossy().into_owned();
    common::run_git(
        main_repo.path(),
        &["worktree", "add", "-b", branch, &worktree_arg, "HEAD"],
    );
    (main_repo, worktree_base, worktree_path)
}

fn add_managed_file(
    repo: &Path,
    store: &Path,
    rel_path: &str,
    contents: &str,
) -> (String, PathBuf) {
    let file_path = repo.join(rel_path);
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&file_path, contents).unwrap();

    let mut ctx = context::build(repo, Some(store), true).unwrap();
    let repo_id = ctx.repo_id.clone();
    let repo_store = ctx.repo_store.clone();
    ops::add::add(
        &mut ctx,
        &file_path,
        false,
        &DefaultLinkStrategy,
        &GitInfoExclude,
    )
    .unwrap();
    (repo_id, repo_store)
}

fn sample_item(item_id: &str, repo_id: &str, path: &str, state: OwnershipState) -> Item {
    Item {
        item_id: item_id.to_string(),
        origin_repo_id: repo_id.to_string(),
        path: path.to_string(),
        store_path: format!("items/{path}"),
        ownership_state: state,
        created_at: "2026-04-29T00:00:00Z".into(),
        updated_at: "2026-04-29T00:00:00Z".into(),
    }
}

fn write_manifest(store: &Path, repo_store_dir: &str, manifest: &Manifest) -> PathBuf {
    let repo_store = store.join("repos").join(repo_store_dir);
    for item in &manifest.items {
        let store_path = repo_store.join(&item.store_path);
        if let Some(parent) = store_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&store_path, format!("data for {}", item.path)).unwrap();
    }
    manifest::save(&repo_store, manifest).unwrap();
    repo_store
}

#[test]
fn move_repository_path_reuses_repoid_via_git_common_dir() {
    if !require_symlink_support() {
        return;
    }
    let (_main_repo, worktree_base, worktree) = linked_worktree("move-path", "app");
    let store = TempDir::new().unwrap();

    let (repo_id, _) = add_managed_file(&worktree, store.path(), "first.env", "one");
    let original_common = context::current_git_context(&worktree)
        .unwrap()
        .git_common_dir;

    let moved_parent = worktree_base.path().join("moved");
    std::fs::create_dir(&moved_parent).unwrap();
    let moved_worktree = moved_parent.join("app");
    std::fs::rename(&worktree, &moved_worktree).unwrap();

    let next = moved_worktree.join("second.env");
    std::fs::write(&next, "two").unwrap();
    let mut moved_ctx = context::build(&moved_worktree, Some(store.path()), true).unwrap();
    assert_eq!(moved_ctx.repo_id, repo_id);
    assert_eq!(moved_ctx.git_common_dir, original_common);

    ops::add::add(
        &mut moved_ctx,
        &next,
        false,
        &DefaultLinkStrategy,
        &GitInfoExclude,
    )
    .unwrap();

    let idx = index::load(store.path()).unwrap();
    let entry = idx.get(&repo_id).unwrap();
    assert_eq!(entry.root.as_deref(), Some(moved_worktree.as_path()));
    assert_eq!(moved_ctx.manifest.items.len(), 2);
}

#[test]
fn rename_repository_directory_reuses_repoid_via_git_common_dir() {
    if !require_symlink_support() {
        return;
    }
    let (_main_repo, worktree_base, worktree) = linked_worktree("rename-path", "app");
    let store = TempDir::new().unwrap();

    let (repo_id, _) = add_managed_file(&worktree, store.path(), "first.env", "one");
    let original_common = context::current_git_context(&worktree)
        .unwrap()
        .git_common_dir;

    let renamed_worktree = worktree_base.path().join("app-renamed");
    std::fs::rename(&worktree, &renamed_worktree).unwrap();

    let next = renamed_worktree.join("second.env");
    std::fs::write(&next, "two").unwrap();
    let mut renamed_ctx = context::build(&renamed_worktree, Some(store.path()), true).unwrap();
    assert_eq!(renamed_ctx.repo_id, repo_id);
    assert_eq!(renamed_ctx.git_common_dir, original_common);

    ops::add::add(
        &mut renamed_ctx,
        &next,
        false,
        &DefaultLinkStrategy,
        &GitInfoExclude,
    )
    .unwrap();

    let idx = index::load(store.path()).unwrap();
    let entry = idx.get(&repo_id).unwrap();
    assert_eq!(entry.root.as_deref(), Some(renamed_worktree.as_path()));
    assert_eq!(renamed_ctx.manifest.items.len(), 2);
}

#[test]
fn renamed_repo_store_dir_rebuild_index_restores_locator_and_repair_succeeds() {
    if !require_symlink_support() {
        return;
    }
    let repo = common::init_git_repo();
    let store = TempDir::new().unwrap();
    let (repo_id, repo_store) = add_managed_file(repo.path(), store.path(), "secret.env", "secret");
    let old_store_dir = repo_store
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let new_store_dir = format!("{old_store_dir}-renamed");
    let new_repo_store = store.path().join("repos").join(&new_store_dir);

    std::fs::rename(&repo_store, &new_repo_store).unwrap();

    let report = ops::rebuild_index::run(store.path(), false).unwrap();
    assert_eq!(report.repositories, 1);
    let rebuilt = index::load(store.path()).unwrap();
    let rebuilt_entry = rebuilt.get(&repo_id).unwrap();
    assert_eq!(rebuilt_entry.repo_store_dir, new_store_dir);
    assert_eq!(rebuilt_entry.root, None);

    let current = context::current_git_context(repo.path()).unwrap();
    ops::reclaim::execute_reclaim(store.path(), &current, &repo_id).unwrap();

    let repo_file = repo.path().join("secret.env");
    std::fs::remove_file(&repo_file).unwrap();
    GitInfoExclude
        .remove_entries(repo.path(), &["secret.env"])
        .unwrap();

    let mut ctx = context::build(repo.path(), Some(store.path()), true).unwrap();
    let repair = ops::repair::repair_repo(&mut ctx, &DefaultLinkStrategy, false, false).unwrap();

    assert_eq!(repair.symlinks_repaired, 1);
    assert!(repair.exclude_updated);
    assert!(DefaultLinkStrategy.is_managed_link(&repo_file, store.path()));
    assert!(GitInfoExclude.has_entry(repo.path(), "secret.env").unwrap());
}

#[test]
fn delete_index_and_rebuild_restores_repoid_and_store_dir_without_git_metadata() {
    if !require_symlink_support() {
        return;
    }
    let repo = common::init_git_repo();
    let store = TempDir::new().unwrap();
    let (repo_id, repo_store) = add_managed_file(repo.path(), store.path(), "secret.env", "secret");
    let repo_store_dir = repo_store
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    std::fs::remove_file(index::index_path(store.path())).unwrap();
    let report = ops::rebuild_index::run(store.path(), false).unwrap();
    let rebuilt = index::load(store.path()).unwrap();
    let entry = rebuilt.get(&repo_id).unwrap();

    assert_eq!(report.repositories, 1);
    assert_eq!(entry.repo_store_dir, repo_store_dir);
    assert_eq!(entry.root, None);
    assert_eq!(entry.git_dir, None);
    assert_eq!(entry.git_common_dir, None);
}

#[test]
fn reclone_reclaim_associates_existing_repoid_after_fresh_repoid() {
    if !require_symlink_support() {
        return;
    }
    let original = common::init_git_repo();
    common::run_git(
        original.path(),
        &["remote", "add", "origin", "git@github.com:example/app.git"],
    );
    let store = TempDir::new().unwrap();
    let (old_repo_id, _) = add_managed_file(original.path(), store.path(), "secret.env", "secret");

    let reclone = common::init_git_repo();
    common::run_git(
        reclone.path(),
        &["remote", "add", "origin", "git@github.com:example/app.git"],
    );
    let fresh_ctx = context::build(reclone.path(), Some(store.path()), true).unwrap();
    let fresh_repo_id = fresh_ctx.repo_id.clone();
    assert_ne!(fresh_repo_id, old_repo_id);
    ops::reclaim::check_reclaim_precondition(Some(&fresh_ctx.manifest)).unwrap();
    drop(fresh_ctx);

    let current = context::current_git_context(reclone.path()).unwrap();
    let outcome = ops::reclaim::execute_reclaim(store.path(), &current, &old_repo_id).unwrap();
    assert_eq!(outcome.repo_id, old_repo_id);

    let reclaimed_ctx = context::build(reclone.path(), Some(store.path()), false).unwrap();
    assert_eq!(reclaimed_ctx.repo_id, old_repo_id);
    let idx = index::load(store.path()).unwrap();
    assert!(
        idx.get(&fresh_repo_id).is_none(),
        "reclaim must remove the empty throwaway association"
    );
}

#[test]
fn repair_after_reclaim_restores_symlinks_and_exclude_entries() {
    if !require_symlink_support() {
        return;
    }
    let original = common::init_git_repo();
    common::run_git(
        original.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/app.git",
        ],
    );
    let store = TempDir::new().unwrap();
    let (repo_id, _) = add_managed_file(original.path(), store.path(), "secret.env", "secret");

    ops::rebuild_index::run(store.path(), false).unwrap();

    let reclone = common::init_git_repo();
    common::run_git(
        reclone.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/app.git",
        ],
    );
    let current = context::current_git_context(reclone.path()).unwrap();
    ops::reclaim::execute_reclaim(store.path(), &current, &repo_id).unwrap();

    let mut ctx = context::build(reclone.path(), Some(store.path()), true).unwrap();
    let repair = ops::repair::repair_repo(&mut ctx, &DefaultLinkStrategy, false, false).unwrap();
    let repaired_path = reclone.path().join("secret.env");

    assert_eq!(repair.symlinks_repaired, 1);
    assert!(repair.exclude_updated);
    assert!(DefaultLinkStrategy.is_managed_link(&repaired_path, store.path()));
    assert_eq!(std::fs::read_to_string(&repaired_path).unwrap(), "secret");
    assert!(GitInfoExclude
        .has_entry(reclone.path(), "secret.env")
        .unwrap());
}

#[test]
fn reclaim_rejects_current_repo_with_items_before_mutation() {
    if !require_symlink_support() {
        return;
    }
    let target = common::init_git_repo();
    let current = common::init_git_repo();
    let store = TempDir::new().unwrap();
    let (target_repo_id, _) = add_managed_file(target.path(), store.path(), "target.env", "target");

    let mut current_ctx = context::build(current.path(), Some(store.path()), true).unwrap();
    let current_file = current.path().join("current.env");
    std::fs::write(&current_file, "current").unwrap();
    ops::add::add(
        &mut current_ctx,
        &current_file,
        false,
        &DefaultLinkStrategy,
        &GitInfoExclude,
    )
    .unwrap();
    let index_before = std::fs::read_to_string(index::index_path(store.path())).unwrap();

    let result = ops::reclaim::check_reclaim_precondition(Some(&current_ctx.manifest));

    assert!(result.is_err());
    assert_eq!(
        std::fs::read_to_string(index::index_path(store.path())).unwrap(),
        index_before
    );
    assert!(index::load(store.path())
        .unwrap()
        .get(&target_repo_id)
        .is_some());
}

#[test]
fn duplicate_repoid_makes_rebuild_index_and_reclaim_fail_hard() {
    let store = TempDir::new().unwrap();
    write_manifest(
        store.path(),
        "project-a",
        &Manifest::new("repo-duplicate", "2026-04-29T00:00:00Z"),
    );
    write_manifest(
        store.path(),
        "project-b",
        &Manifest::new("repo-duplicate", "2026-04-29T00:00:00Z"),
    );
    let repo = common::init_git_repo();
    let current = context::current_git_context(repo.path()).unwrap();
    let idx = index::Index::new();

    let rebuild = ops::rebuild_index::run(store.path(), false);
    let candidates = ops::reclaim::build_candidates(store.path(), repo.path(), None, &idx);
    let reclaim = ops::reclaim::execute_reclaim(store.path(), &current, "repo-duplicate");

    assert!(rebuild
        .unwrap_err()
        .to_string()
        .contains("duplicate repo_id"));
    assert!(candidates
        .unwrap_err()
        .to_string()
        .contains("duplicate repo_id"));
    assert!(reclaim
        .unwrap_err()
        .to_string()
        .contains("duplicate repo_id"));
    assert!(!index::index_path(store.path()).exists());
}

#[test]
fn duplicate_itemid_makes_rebuild_index_and_reclaim_fail_hard() {
    let store = TempDir::new().unwrap();
    let mut first = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
    first.add(sample_item(
        "item-duplicate",
        "repo-1",
        "one.env",
        OwnershipState::Attached,
    ));
    let mut second = Manifest::new("repo-2", "2026-04-29T00:00:00Z");
    second.add(sample_item(
        "item-duplicate",
        "repo-2",
        "two.env",
        OwnershipState::Attached,
    ));
    write_manifest(store.path(), "project-a", &first);
    write_manifest(store.path(), "project-b", &second);
    let repo = common::init_git_repo();
    let current = context::current_git_context(repo.path()).unwrap();
    let idx = index::Index::new();

    let rebuild = ops::rebuild_index::run(store.path(), false);
    let candidates = ops::reclaim::build_candidates(store.path(), repo.path(), None, &idx);
    let reclaim = ops::reclaim::execute_reclaim(store.path(), &current, "repo-1");

    assert!(rebuild
        .unwrap_err()
        .to_string()
        .contains("duplicate item_id"));
    assert!(candidates
        .unwrap_err()
        .to_string()
        .contains("duplicate item_id"));
    assert!(reclaim
        .unwrap_err()
        .to_string()
        .contains("duplicate item_id"));
    assert!(!index::index_path(store.path()).exists());
}

#[test]
fn gc_does_not_delete_unreachable_repos_or_items() {
    let store = TempDir::new().unwrap();
    let mut mf = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
    mf.add(sample_item(
        "item-unreachable",
        "repo-1",
        "secret.env",
        OwnershipState::Unreachable,
    ));
    let repo_store = write_manifest(store.path(), "project", &mf);

    let report = ops::gc::run(store.path(), false).unwrap();
    let loaded = manifest::load(&repo_store).unwrap();

    assert!(report.candidates.is_empty());
    assert_eq!(report.deleted_items, 0);
    assert!(repo_store.exists());
    assert!(repo_store.join("items/secret.env").exists());
    assert_eq!(loaded.items.len(), 1);
    assert_eq!(loaded.items[0].ownership_state, OwnershipState::Unreachable);
}
