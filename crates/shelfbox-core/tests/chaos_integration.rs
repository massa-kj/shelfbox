/// Chaos-style integration tests for failure scenarios.
///
/// Each test simulates a real-world failure mode (deleted files, missing index,
/// linked worktrees, partial corruption) and verifies that shelfbox detects
/// the problem correctly and recovers where possible.
///
/// These tests complement the happy-path and error-path coverage in
/// `ops_integration.rs`.  See `docs/failure-matrix.md` for the full list of
/// failure modes and their recovery strategies.
use std::process::Command as StdCommand;

use tempfile::TempDir;

use shelfbox_core::{
    context, ignore::GitInfoExclude, link::DefaultLinkStrategy, ops, ops::integrity::FixResult,
};

mod common;

fn require_symlink_support() -> bool {
    common::require_symlink_support()
}

// ── Worktree scenarios (failure matrix #6) ────────────────────────────────────

/// Accessing a repository via a linked worktree must reuse the same ULID as
/// the main clone so that both share a single shelf.
///
/// Mechanism: `git_common_dir` for a linked worktree points to the main
/// clone's `.git/`, which is used as the secondary lookup key in the index.
#[test]
fn worktree_add_reuses_repo_ulid() {
    let main_dir = common::init_git_repo_with_commit();
    let store_dir = TempDir::new().unwrap();

    // Register the main clone in the index.
    let ctx_main = context::build(main_dir.path(), Some(store_dir.path()), false).unwrap();
    let main_repo_id = ctx_main.repo_id.clone();
    drop(ctx_main); // release lock

    // Create a linked worktree inside the store temp dir (path must not exist).
    let wt_path = store_dir.path().join("linked-wt");
    let out = StdCommand::new("git")
        .args(["worktree", "add", wt_path.to_str().unwrap(), "HEAD"])
        .current_dir(main_dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git worktree add failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Build context from the linked worktree.
    let ctx_wt = context::build(&wt_path, Some(store_dir.path()), false).unwrap();

    assert_eq!(
        ctx_wt.repo_id, main_repo_id,
        "linked worktree must reuse the main clone's ULID"
    );
}

/// Items shelved from the main clone must appear in the manifest when the
/// repository is accessed via a linked worktree.
#[test]
fn worktree_shelved_items_visible_from_linked_worktree() {
    if !require_symlink_support() {
        return;
    }
    let main_dir = common::init_git_repo_with_commit();
    let store_dir = TempDir::new().unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    // Shelve a file from the main clone.
    let file_path = main_dir.path().join("secret.env");
    std::fs::write(&file_path, "TOKEN=abc").unwrap();

    let mut ctx = context::build(main_dir.path(), Some(store_dir.path()), true).unwrap();
    ops::add::add(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    drop(ctx);

    // Create a linked worktree.
    let wt_path = store_dir.path().join("linked-wt");
    let out = StdCommand::new("git")
        .args(["worktree", "add", wt_path.to_str().unwrap(), "HEAD"])
        .current_dir(main_dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git worktree add failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Build context from the linked worktree.
    let ctx_wt = context::build(&wt_path, Some(store_dir.path()), false).unwrap();

    // The manifest is shared; the item shelved from the main clone must appear.
    assert_eq!(
        ctx_wt.manifest.items.len(),
        1,
        "manifest must contain the item shelved from the main clone"
    );
    assert_eq!(ctx_wt.manifest.items[0].path, "secret.env");
}

// ── Index lost (failure matrix #4) ───────────────────────────────────────────

/// When `index.json` is deleted, `context::build` creates a fresh index entry
/// with a new ULID.  The old store directory is untouched (no data loss), but
/// it becomes an orphan unreachable via the new context.
#[test]
fn index_deleted_creates_fresh_context_with_empty_manifest() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    // Shelve a file to populate the store.
    let file_path = repo_dir.path().join("secret.txt");
    std::fs::write(&file_path, "secret").unwrap();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path()), true).unwrap();
    ops::add::add(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    let original_repo_id = ctx.repo_id.clone();
    let original_store = ctx.repo_store.clone();
    drop(ctx);

    // Sanity: store-side file exists before deleting the index.
    assert!(
        original_store.join("items/secret.txt").exists(),
        "store-side file must exist before index deletion"
    );

    // Simulate index loss.
    let index_path = store_dir.path().join("index.json");
    std::fs::remove_file(&index_path).unwrap();

    // Rebuild context: empty index → new ULID assigned.
    let ctx_fresh = context::build(repo_dir.path(), Some(store_dir.path()), false).unwrap();

    assert_ne!(
        ctx_fresh.repo_id, original_repo_id,
        "a fresh index means a new ULID is assigned to the same repo"
    );

    // The fresh manifest is empty (new repo store, no items recorded).
    assert!(
        ctx_fresh.manifest.items.is_empty(),
        "fresh context must have an empty manifest"
    );

    // Store-side file in the OLD repo store is untouched (no data loss).
    assert!(
        original_store.join("items/secret.txt").exists(),
        "store-side file must survive index deletion"
    );

    // The repo-side symlink still exists (points to old store path).
    assert!(
        file_path
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "repo-side symlink must remain intact after index deletion"
    );
}

// ── Concurrent read access (failure matrix #9) ───────────────────────────────

/// Multiple read-only contexts on the same store must coexist without
/// blocking each other (shared `flock` mode).
#[test]
fn concurrent_read_locks_are_shared() {
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    // Two read-only contexts can be held simultaneously.
    let ctx1 = context::build(repo_dir.path(), Some(store_dir.path()), false).unwrap();
    let ctx2 = context::build(repo_dir.path(), Some(store_dir.path()), false).unwrap();

    assert_eq!(
        ctx1.repo_id, ctx2.repo_id,
        "both read contexts must see the same repo"
    );

    drop(ctx1);
    drop(ctx2);
}

// ── Partial store corruption (failure matrix #10) ─────────────────────────────

/// When some store items are deleted while others remain intact, `repo status`
/// must report a mixed result: healthy items alongside irrecoverable ones.
/// `repo repair` must record `CannotFix` for the missing items without
/// touching the healthy ones.
#[test]
fn partial_store_corruption_shows_mixed_status() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    // Shelve three files.
    let names = ["alpha.txt", "beta.txt", "gamma.txt"];
    let paths: Vec<_> = names
        .iter()
        .map(|name| {
            let p = repo_dir.path().join(name);
            std::fs::write(&p, *name).unwrap();
            p
        })
        .collect();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path()), true).unwrap();
    for p in &paths {
        ops::add::add(&mut ctx, p, false, &link, &ignore).unwrap();
    }

    // Corrupt one store item (simulate partial copy or deletion).
    std::fs::remove_file(ctx.repo_store.join("items/beta.txt")).unwrap();

    let report = ops::integrity::check(&ctx, &link, &ignore).unwrap();

    assert_eq!(
        report.items.len(),
        3,
        "all three manifest items must be reported"
    );

    let healthy_count = report.items.iter().filter(|s| s.ok).count();
    let broken_count = report.items.iter().filter(|s| !s.ok).count();
    assert_eq!(healthy_count, 2, "two items must be healthy");
    assert_eq!(broken_count, 1, "one item must be broken");

    let broken = report
        .items
        .iter()
        .find(|s| !s.ok)
        .expect("broken item must exist");
    assert!(
        !broken.store_exists,
        "broken item must have store_exists: false"
    );

    // doctor --fix must record CannotFix for the missing item.
    let fix_report = ops::integrity::fix(&mut ctx, &link, &ignore, false, false).unwrap();
    assert!(
        fix_report
            .actions
            .iter()
            .any(|a| matches!(a, FixResult::CannotFix(_))),
        "doctor must emit CannotFix for the missing store item"
    );
    assert!(
        !fix_report.data_loss_warnings.is_empty(),
        "data_loss_warnings must be populated for the missing item"
    );

    // The two healthy items must remain accessible.
    assert_eq!(
        std::fs::read_to_string(&paths[0]).unwrap(),
        "alpha.txt",
        "alpha.txt must still be readable"
    );
    assert_eq!(
        std::fs::read_to_string(&paths[2]).unwrap(),
        "gamma.txt",
        "gamma.txt must still be readable"
    );
}
