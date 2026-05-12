/// Integration tests for the ops layer.
///
/// Each test spins up a real Git repository in a temp directory and exercises
/// the core operations end-to-end using real file I/O and (where required)
/// real Git subprocesses.  No mocking is used; the tests verify the full
/// interaction between context, manifest, link strategy and ignore backend.
use std::process::Command as StdCommand;

use tempfile::TempDir;

use shelfbox_core::{
    context,
    ignore::{GitInfoExclude, IgnoreBackend},
    link::{LinkStrategy, SymlinkStrategy},
    ops,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Creates a minimal Git repository in a temp directory and returns it.
fn init_git_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    let path = dir.path();

    StdCommand::new("git")
        .args(["init", "-b", "main"])
        .current_dir(path)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(path)
        .output()
        .unwrap();

    dir
}

// ── add / restore ─────────────────────────────────────────────────────────────

#[test]
fn add_and_restore_file() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    // Create an untracked file to shelve.
    let file_path = repo_dir.path().join("secret.txt");
    std::fs::write(&file_path, "sensitive data").unwrap();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    // --- add ---
    ops::add::add(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    // Original path should now be a symlink.
    assert!(
        file_path
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "expected a symlink at the repo path after add"
    );
    // Contents accessible through the symlink.
    assert_eq!(
        std::fs::read_to_string(&file_path).unwrap(),
        "sensitive data"
    );

    // Store-side file must exist.
    let store_path = ctx.repo_store.join("items/secret.txt");
    assert!(store_path.exists(), "store-side file must exist");

    // Manifest must reflect the addition.
    assert_eq!(ctx.manifest.items.len(), 1);
    assert_eq!(ctx.manifest.items[0].path, "secret.txt");

    // --- list ---
    let items = ops::list::list(&ctx);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].path, "secret.txt");

    // --- status: everything should be healthy ---
    let statuses = ops::status::status(&ctx, &link, &ignore).unwrap();
    assert_eq!(statuses.len(), 1);
    assert!(statuses[0].ok, "status should be ok after add");

    // --- restore ---
    ops::restore::restore(&mut ctx, &file_path, false, false, &link, &ignore).unwrap();

    // Original path should now be a regular file again.
    let restored_meta = file_path.symlink_metadata().unwrap();
    assert!(
        !restored_meta.file_type().is_symlink(),
        "expected a regular file at the repo path after restore"
    );
    assert_eq!(
        std::fs::read_to_string(&file_path).unwrap(),
        "sensitive data"
    );

    // Store-side file must be gone.
    assert!(
        !store_path.exists(),
        "store-side file must be removed after restore"
    );

    // Manifest must be empty.
    assert!(ctx.manifest.items.is_empty());
}

#[test]
fn add_dry_run_makes_no_changes() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("config.toml");
    std::fs::write(&file_path, "[settings]").unwrap();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add(&mut ctx, &file_path, true, &link, &ignore).unwrap();

    // File must remain a regular file.
    assert!(
        !file_path
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "dry-run must not create a symlink"
    );
    // Manifest must be unchanged.
    assert!(
        ctx.manifest.items.is_empty(),
        "dry-run must not modify the manifest"
    );
    // Store must have no items directory.
    assert!(
        !ctx.repo_store.join("items").exists(),
        "dry-run must not create store items"
    );
}

#[test]
fn restore_dry_run_makes_no_changes() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("notes.md");
    std::fs::write(&file_path, "# notes").unwrap();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    // Actually shelve the file first.
    ops::add::add(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    assert_eq!(ctx.manifest.items.len(), 1);

    // Dry-run restore.
    ops::restore::restore(&mut ctx, &file_path, true, false, &link, &ignore).unwrap();

    // Symlink must still be in place.
    assert!(
        file_path
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "dry-run restore must not remove the symlink"
    );
    // Manifest must still contain the item.
    assert_eq!(ctx.manifest.items.len(), 1);
}

#[test]
fn add_already_managed_returns_error() {
    // Simulate an inconsistent state: the manifest records the item but the
    // symlink was removed and the original file was copied back manually.
    // In this state the path is a regular file, not a symlink, so the
    // IsSymlink check passes, but the manifest check must fire.
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("data.txt");
    std::fs::write(&file_path, "data").unwrap();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    // Shelve the file normally.
    ops::add::add(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    // Simulate inconsistency: remove the symlink and put a regular file back,
    // but leave the manifest entry in place.
    let store_path = ctx.repo_store.join("items/data.txt");
    std::fs::remove_file(&file_path).unwrap(); // remove symlink
    std::fs::copy(&store_path, &file_path).unwrap(); // restore regular file

    // A second add on the regular file must fail with AlreadyManaged because
    // the manifest still contains the entry.
    let err = ops::add::add(&mut ctx, &file_path, false, &link, &ignore).unwrap_err();
    assert!(
        matches!(err, shelfbox_core::error::AppError::AlreadyManaged { .. }),
        "expected AlreadyManaged, got: {err}"
    );
}

#[test]
fn add_path_outside_repo_returns_error() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let outside_file = store_dir.path().join("outside.txt");
    std::fs::write(&outside_file, "outside").unwrap();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    let err = ops::add::add(&mut ctx, &outside_file, false, &link, &ignore).unwrap_err();
    assert!(
        matches!(err, shelfbox_core::error::AppError::PathOutsideRepo { .. }),
        "expected PathOutsideRepo, got: {err}"
    );
}

#[test]
fn restore_not_managed_link_returns_error() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("plain.txt");
    std::fs::write(&file_path, "plain").unwrap();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    // Trying to restore a file that was never shelved must fail.
    let err =
        ops::restore::restore(&mut ctx, &file_path, false, false, &link, &ignore).unwrap_err();
    assert!(
        matches!(err, shelfbox_core::error::AppError::NotManagedLink { .. }),
        "expected NotManagedLink, got: {err}"
    );
}

// ── keep_ignore ---

#[test]
fn restore_keep_ignore_preserves_exclude_entry() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("env.sh");
    std::fs::write(&file_path, "export SECRET=1").unwrap();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    assert!(ignore.has_entry(repo_dir.path(), "env.sh").unwrap());

    // Restore with keep_ignore=true.
    ops::restore::restore(&mut ctx, &file_path, false, true, &link, &ignore).unwrap();

    // Entry must still be present.
    assert!(
        ignore.has_entry(repo_dir.path(), "env.sh").unwrap(),
        "keep_ignore=true must preserve the exclude entry"
    );
}

// ── doctor ────────────────────────────────────────────────────────────────────

#[test]
fn doctor_finds_orphan_store_item() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("orphan_test.txt");
    std::fs::write(&file_path, "test").unwrap();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    // Shelve the file, then manually inject an orphan file into the store.
    ops::add::add(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    let orphan_path = ctx.items_dir().join("orphan_injected.txt");
    std::fs::write(&orphan_path, "orphan").unwrap();

    let report = ops::doctor::doctor(&ctx, &link, &ignore).unwrap();

    assert_eq!(report.items.len(), 1);
    assert!(report.items[0].ok, "managed item must be reported as ok");
    assert_eq!(report.orphan_store_items.len(), 1);
    assert_eq!(report.orphan_store_items[0], "orphan_injected.txt");
}

#[test]
fn doctor_empty_repo_is_clean() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    let report = ops::doctor::doctor(&ctx, &link, &ignore).unwrap();

    assert!(report.items.is_empty());
    assert!(report.orphan_store_items.is_empty());
    assert!(
        report.repo_root_matches_index,
        "repo root should match index for a freshly built context"
    );
}

// ── add validation edge cases ─────────────────────────────────────────────────

#[test]
fn add_tracked_file_returns_error() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    // Create, stage and commit a file so it is tracked by Git.
    let file_path = repo_dir.path().join("tracked.txt");
    std::fs::write(&file_path, "contents").unwrap();
    StdCommand::new("git")
        .args(["add", "tracked.txt"])
        .current_dir(repo_dir.path())
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-m", "add tracked file"])
        .current_dir(repo_dir.path())
        .output()
        .unwrap();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    let err = ops::add::add(&mut ctx, &file_path, false, &link, &ignore).unwrap_err();
    assert!(
        matches!(err, shelfbox_core::error::AppError::PathIsTracked { .. }),
        "expected PathIsTracked, got: {err}"
    );
}

#[test]
fn add_git_dir_path_returns_error() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    // Target a file inside .git/ (e.g. .git/config, which always exists).
    let git_config = repo_dir.path().join(".git").join("config");

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    let err = ops::add::add(&mut ctx, &git_config, false, &link, &ignore).unwrap_err();
    assert!(
        matches!(err, shelfbox_core::error::AppError::PathInsideGitDir { .. }),
        "expected PathInsideGitDir, got: {err}"
    );
}

#[test]
fn add_existing_symlink_returns_error() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    // Create a symlink that is not managed by shelfbox.
    let target = repo_dir.path().join("target_file.txt");
    std::fs::write(&target, "target").unwrap();
    let link_path = repo_dir.path().join("my_link");
    std::os::unix::fs::symlink(&target, &link_path).unwrap();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    let err = ops::add::add(&mut ctx, &link_path, false, &link, &ignore).unwrap_err();
    assert!(
        matches!(err, shelfbox_core::error::AppError::PathIsSymlink { .. }),
        "expected PathIsSymlink, got: {err}"
    );
}

// ── doctor status checks ──────────────────────────────────────────────────────

#[test]
fn doctor_reports_error_for_dangling_symlink() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("secrets.txt");
    std::fs::write(&file_path, "secret").unwrap();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    // Remove the store-side file to create a dangling symlink.
    let store_path = ctx.repo_store.join("items/secrets.txt");
    std::fs::remove_file(&store_path).unwrap();

    let report = ops::doctor::doctor(&ctx, &link, &ignore).unwrap();

    assert_eq!(report.items.len(), 1);
    let s = &report.items[0];
    assert!(s.link_exists, "symlink node still present at repo side");
    assert!(!s.store_exists, "store item was removed; should be false");
    assert!(!s.ok, "item with missing store should not be ok");
}

#[test]
fn doctor_reports_warn_for_missing_exclude_entry() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("private.txt");
    std::fs::write(&file_path, "private").unwrap();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    // Manually remove the exclude entry to simulate a WARN condition.
    ignore
        .remove_entries(repo_dir.path(), &["private.txt"])
        .unwrap();

    let report = ops::doctor::doctor(&ctx, &link, &ignore).unwrap();

    assert_eq!(report.items.len(), 1);
    let s = &report.items[0];
    assert!(s.link_exists, "symlink must still exist");
    assert!(s.link_valid, "symlink must still be valid");
    assert!(s.store_exists, "store item must still exist");
    assert!(!s.in_exclude, "exclude entry was removed; should be false");
    assert!(!s.ok, "item missing from exclude should not be ok");
}

// ── repair ────────────────────────────────────────────────────────────────────

#[test]
fn repair_recreates_missing_symlink() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("secret.env");
    std::fs::write(&file_path, "TOKEN=abc").unwrap();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    assert!(file_path
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());

    // Simulate the symlink being removed (e.g. by `rm`).
    std::fs::remove_file(&file_path).unwrap();
    assert!(!file_path.exists(), "symlink must be gone before repair");

    let outcome = ops::repair::repair(&ctx, &file_path, &link, false).unwrap();
    assert_eq!(outcome, ops::repair::RepairOutcome::LinkRecreated);

    // Symlink must be back and readable.
    assert!(file_path
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "TOKEN=abc");
}

#[test]
fn repair_relinks_invalid_symlink() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("cfg.toml");
    std::fs::write(&file_path, "[db]").unwrap();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    // Replace managed symlink with one pointing elsewhere (hand-modified).
    std::fs::remove_file(&file_path).unwrap();
    std::os::unix::fs::symlink("/tmp/nonexistent", &file_path).unwrap();
    assert!(
        !link.is_managed_link(&file_path, &ctx.config.store),
        "symlink must be invalid before repair"
    );

    let outcome = ops::repair::repair(&ctx, &file_path, &link, false).unwrap();
    assert_eq!(outcome, ops::repair::RepairOutcome::LinkRecreated);

    assert!(link.is_managed_link(&file_path, &ctx.config.store));
    assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "[db]");
}

#[test]
fn repair_already_healthy_returns_no_op() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("healthy.txt");
    std::fs::write(&file_path, "ok").unwrap();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    let outcome = ops::repair::repair(&ctx, &file_path, &link, false).unwrap();
    assert_eq!(outcome, ops::repair::RepairOutcome::AlreadyHealthy);
}

#[test]
fn repair_returns_store_missing_when_store_item_gone() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("secrets.txt");
    std::fs::write(&file_path, "secret").unwrap();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    // Delete the store-side copy to simulate data loss.
    let store_item = ctx.repo_store.join("items/secrets.txt");
    std::fs::remove_file(&store_item).unwrap();

    let outcome = ops::repair::repair(&ctx, &file_path, &link, false).unwrap();
    assert_eq!(outcome, ops::repair::RepairOutcome::StoreMissing);

    // The (now dangling) symlink must be left untouched.
    assert!(
        file_path.symlink_metadata().is_ok(),
        "repair must not remove the dangling symlink"
    );
}

#[test]
fn repair_returns_not_managed_for_unknown_path() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let unmanaged = repo_dir.path().join("unmanaged.txt");
    std::fs::write(&unmanaged, "not shelved").unwrap();

    let ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;

    let outcome = ops::repair::repair(&ctx, &unmanaged, &link, false).unwrap();
    assert_eq!(outcome, ops::repair::RepairOutcome::NotManaged);
}

#[test]
fn repair_refuses_to_overwrite_regular_file() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("data.txt");
    std::fs::write(&file_path, "original").unwrap();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    // Remove the symlink and put a regular file back in its place.
    std::fs::remove_file(&file_path).unwrap();
    std::fs::write(&file_path, "user placed file").unwrap();
    assert!(
        !file_path
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "must be a regular file before the safety check"
    );

    let result = ops::repair::repair(&ctx, &file_path, &link, false);
    assert!(
        result.is_err(),
        "repair must return an error to prevent data loss"
    );

    // The user's file must be intact.
    assert_eq!(
        std::fs::read_to_string(&file_path).unwrap(),
        "user placed file"
    );
}

#[test]
fn repair_dry_run_makes_no_changes() {
    let repo_dir = init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("dryrun.txt");
    std::fs::write(&file_path, "contents").unwrap();

    let mut ctx = context::build(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    std::fs::remove_file(&file_path).unwrap();

    let outcome = ops::repair::repair(&ctx, &file_path, &link, true).unwrap();
    assert_eq!(outcome, ops::repair::RepairOutcome::LinkRecreated);

    // Symlink must NOT have been recreated in dry-run mode.
    assert!(!file_path.exists(), "dry-run must not recreate the symlink");
}
