/// End-to-end scenario tests covering realistic user workflows.
///
/// These tests exercise multi-step operations that span context lifecycle,
/// file system changes, and concurrency, verifying that shelfbox behaves
/// correctly across common real-world sequences.
///
/// See `docs/failure-matrix.md` for the failure modes each scenario targets.
use std::collections::HashSet;

use tempfile::TempDir;

use shelfbox_core::{context, ignore::GitInfoExclude, link::DefaultLinkStrategy, ops};

use crate::integration_test_common as common;

fn require_symlink_support() -> bool {
    common::require_symlink_support()
}

// ── Scenario 1: re-clone ──────────────────────────────────────────────────────

/// Deleting and re-cloning a repository must assign a *new* ULID while leaving
/// the original store directory untouched.
///
/// Failure matrix: #4 (repo moved / re-cloned).
#[test]
fn reclone_starts_fresh_while_preserving_old_store() {
    if !require_symlink_support() {
        return;
    }
    let original_repo = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    // Shelve a file in the original clone.
    let secret = original_repo.path().join("secret.txt");
    std::fs::write(&secret, "sensitive data").unwrap();

    let mut ctx =
        context::build_create_or_load(original_repo.path(), Some(store_dir.path())).unwrap();
    let original_id = ctx.repo_id.clone();
    let original_store = ctx.repo_store.clone();
    ops::add::add_report(
        &mut ctx,
        &secret,
        false,
        &DefaultLinkStrategy,
        &GitInfoExclude,
    )
    .unwrap();
    drop(ctx);

    // Verify the store item exists on disk.
    assert!(
        original_store.join("items").join("secret.txt").exists(),
        "shelved file must be in store"
    );

    // Simulate a re-clone by creating a brand-new repository (new git_common_dir).
    let recloned_repo = common::init_git_repo();
    let ctx_reclone =
        context::build_create_or_load(recloned_repo.path(), Some(store_dir.path())).unwrap();

    assert_ne!(
        ctx_reclone.repo_id, original_id,
        "re-cloned repo must receive a new ULID"
    );

    // The manifest for the new clone must be empty.
    let manifest =
        ops::status::status(&ctx_reclone, &DefaultLinkStrategy, &GitInfoExclude).unwrap();
    assert!(
        manifest.is_empty(),
        "re-cloned repo must start with empty manifest"
    );

    // The original store directory must still exist.
    assert!(
        original_store.join("items").join("secret.txt").exists(),
        "original store must be preserved after re-clone"
    );
}

#[test]
fn matching_remote_does_not_reclaim_repo_id_automatically() {
    if !require_symlink_support() {
        return;
    }
    let original_repo = common::init_git_repo();
    common::run_git(
        original_repo.path(),
        &["remote", "add", "origin", "git@github.com:example/app.git"],
    );
    let store_dir = TempDir::new().unwrap();

    let secret = original_repo.path().join("secret.txt");
    std::fs::write(&secret, "sensitive data").unwrap();

    let mut ctx =
        context::build_create_or_load(original_repo.path(), Some(store_dir.path())).unwrap();
    let original_id = ctx.repo_id.clone();
    ops::add::add_report(
        &mut ctx,
        &secret,
        false,
        &DefaultLinkStrategy,
        &GitInfoExclude,
    )
    .unwrap();
    assert_eq!(
        ctx.manifest.identity_hints.remote_hints,
        vec!["github.com/example/app"]
    );
    drop(ctx);

    let recloned_repo = common::init_git_repo();
    common::run_git(
        recloned_repo.path(),
        &["remote", "add", "origin", "git@github.com:example/app.git"],
    );
    let ctx_reclone =
        context::build_create_or_load(recloned_repo.path(), Some(store_dir.path())).unwrap();

    assert_ne!(
        ctx_reclone.repo_id, original_id,
        "matching remote hints must not trigger automatic reclaim"
    );
}

#[test]
fn item_add_updates_identity_hints_without_absolute_paths() {
    if !require_symlink_support() {
        return;
    }
    let repo = common::init_git_repo();
    common::run_git(
        repo.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/app.git",
        ],
    );
    let store_dir = TempDir::new().unwrap();

    let secret = repo.path().join("secret.txt");
    std::fs::write(&secret, "sensitive data").unwrap();

    let mut ctx = context::build_create_or_load(repo.path(), Some(store_dir.path())).unwrap();
    ops::add::add_report(
        &mut ctx,
        &secret,
        false,
        &DefaultLinkStrategy,
        &GitInfoExclude,
    )
    .unwrap();

    let repo_name = repo
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        ctx.manifest.identity_hints.remote_hints,
        vec!["github.com/example/app"]
    );
    assert_eq!(
        ctx.manifest.identity_hints.repo_name_hints.first(),
        Some(&repo_name)
    );

    let repo_path = repo.path().to_string_lossy();
    let store_path = store_dir.path().to_string_lossy();
    for hint in ctx
        .manifest
        .identity_hints
        .remote_hints
        .iter()
        .chain(ctx.manifest.identity_hints.repo_name_hints.iter())
    {
        assert!(!hint.contains(repo_path.as_ref()));
        assert!(!hint.contains(store_path.as_ref()));
        assert!(!std::path::Path::new(hint).is_absolute());
    }
}

// ── Scenario 2: repo rename ───────────────────────────────────────────────────

/// Renaming a repository directory on disk must be detected as a new
/// repository (both `root` and `git_common_dir` change), leaving the original
/// store intact.
///
/// Failure matrix: #4 (repo moved — full rename, not just root update).
#[test]
fn repo_rename_creates_new_index_entry_and_preserves_store() {
    if !require_symlink_support() {
        return;
    }
    // Use a base TempDir and manage subdirectories manually so we can rename.
    let base_dir = TempDir::new().unwrap();
    let api_path = base_dir.path().join("api");
    std::fs::create_dir(&api_path).unwrap();
    common::init_git_repo_at(&api_path);

    let store_dir = TempDir::new().unwrap();

    // Shelve a file in the original directory.
    let secret = api_path.join("secret.txt");
    std::fs::write(&secret, "api secret").unwrap();

    let mut ctx = context::build_create_or_load(&api_path, Some(store_dir.path())).unwrap();
    let original_id = ctx.repo_id.clone();
    let original_store = ctx.repo_store.clone();
    ops::add::add_report(
        &mut ctx,
        &secret,
        false,
        &DefaultLinkStrategy,
        &GitInfoExclude,
    )
    .unwrap();
    drop(ctx);

    // Rename the repository directory.
    let renamed_path = base_dir.path().join("api-renamed");
    std::fs::rename(&api_path, &renamed_path).unwrap();

    // Building a context from the renamed path yields a new ULID because both
    // `root` and `git_common_dir` changed.
    let ctx_renamed = context::build_create_or_load(&renamed_path, Some(store_dir.path())).unwrap();

    assert_ne!(
        ctx_renamed.repo_id, original_id,
        "renamed repo must receive a new ULID"
    );

    // The renamed repo has an empty manifest.
    let manifest =
        ops::status::status(&ctx_renamed, &DefaultLinkStrategy, &GitInfoExclude).unwrap();
    assert!(
        manifest.is_empty(),
        "renamed repo must start with empty manifest"
    );

    // The original store items must still exist.
    assert!(
        original_store.join("items").join("secret.txt").exists(),
        "original store must be preserved after rename"
    );
}

// ── Scenario 3: concurrent adds ───────────────────────────────────────────────

/// Two threads each shelve one file into the same repository concurrently.
/// The advisory write lock ensures the operations are serialised and both
/// items appear in the final manifest.
///
/// Failure matrix: #8 (concurrent access).
#[test]
fn concurrent_adds_serialize_via_lock() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    // Prepare two files before acquiring any context.
    let file1 = repo_dir.path().join("secret1.txt");
    let file2 = repo_dir.path().join("secret2.txt");
    std::fs::write(&file1, "data 1").unwrap();
    std::fs::write(&file2, "data 2").unwrap();

    // Initialize the store once (creates meta.json and index.json) so that
    // concurrent builds below do not race on first-time store creation.
    {
        let _ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    }

    // Clone paths for the background thread.
    let repo_path = repo_dir.path().to_path_buf();
    let store_path = store_dir.path().to_path_buf();
    let file2_path = file2.clone();

    let handle = std::thread::spawn(move || {
        let mut ctx = context::build_create_or_load(&repo_path, Some(&store_path)).unwrap();
        ops::add::add_report(
            &mut ctx,
            &file2_path,
            false,
            &DefaultLinkStrategy,
            &GitInfoExclude,
        )
        .unwrap();
    });

    // Main thread shelves the first file (may block briefly while the other
    // thread holds the exclusive lock).
    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    ops::add::add_report(
        &mut ctx,
        &file1,
        false,
        &DefaultLinkStrategy,
        &GitInfoExclude,
    )
    .unwrap();
    drop(ctx);

    handle.join().expect("background thread must not panic");

    // Both files must appear in the final manifest.
    let ctx_read = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let manifest = ops::status::status(&ctx_read, &DefaultLinkStrategy, &GitInfoExclude).unwrap();

    let names: HashSet<String> = manifest
        .iter()
        .map(|e| {
            std::path::Path::new(&e.path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();

    assert!(
        names.contains("secret1.txt"),
        "secret1.txt must be in manifest"
    );
    assert!(
        names.contains("secret2.txt"),
        "secret2.txt must be in manifest"
    );
}
