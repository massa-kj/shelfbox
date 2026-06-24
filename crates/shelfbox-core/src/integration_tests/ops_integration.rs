/// Integration tests for the ops layer.
///
/// Each test spins up a real Git repository in a temp directory and exercises
/// the core operations end-to-end using real file I/O and (where required)
/// real Git subprocesses.  No mocking is used; the tests verify the full
/// interaction between context, manifest, link strategy and ignore backend.
use tempfile::TempDir;

use shelfbox_core::{
    context,
    ignore::{GitInfoExclude, IgnoreBackend},
    link::{DefaultLinkStrategy, LinkStrategy},
    ops,
    ops::integrity::FixResult,
    plan::item_restore::ItemRestoreAction,
    plan::repo_repair::RepoRepairSymlinkAction,
    store,
};

use crate::integration_test_common as common;

fn require_symlink_support() -> bool {
    common::require_symlink_support()
}

// ── add / restore ─────────────────────────────────────────────────────────────

#[test]
fn add_and_restore_file() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    // Create an untracked file to shelve.
    let file_path = repo_dir.path().join("secret.txt");
    std::fs::write(&file_path, "sensitive data").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    // --- add ---
    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();

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
    ops::restore::restore(&mut ctx, &file_path, false, false, false, &link, &ignore).unwrap();
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
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("config.toml");
    std::fs::write(&file_path, "[settings]").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    let repo_before = common::snapshot_tree(repo_dir.path());
    let store_before = common::snapshot_tree(store_dir.path());

    ops::add::add_report(&mut ctx, &file_path, true, &link, &ignore).unwrap();

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
    assert_eq!(common::snapshot_tree(repo_dir.path()), repo_before);
    assert_eq!(common::snapshot_tree(store_dir.path()), store_before);
}

#[test]
fn restore_dry_run_makes_no_changes() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("notes.md");
    std::fs::write(&file_path, "# notes").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    // Actually shelve the file first.
    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    assert_eq!(ctx.manifest.items.len(), 1);
    let repo_before = common::snapshot_tree(repo_dir.path());
    let store_before = common::snapshot_tree(store_dir.path());

    // Dry-run restore.
    let report =
        ops::restore::restore(&mut ctx, &file_path, true, false, false, &link, &ignore).unwrap();
    assert!(report.dry_run);
    assert_eq!(report.plan.path, "notes.md");
    assert_eq!(report.plan.action, ItemRestoreAction::RestoreFile);

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
    assert_eq!(common::snapshot_tree(repo_dir.path()), repo_before);
    assert_eq!(common::snapshot_tree(store_dir.path()), store_before);
}

#[test]
fn add_already_managed_returns_error() {
    if !require_symlink_support() {
        return;
    }
    // Simulate an inconsistent state: the manifest records the item but the
    // symlink was removed and the original file was copied back manually.
    // In this state the path is a regular file, not a symlink, so the
    // IsSymlink check passes, but the manifest check must fire.
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("data.txt");
    std::fs::write(&file_path, "data").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    // Shelve the file normally.
    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    // Simulate inconsistency: remove the symlink and put a regular file back,
    // but leave the manifest entry in place.
    let store_path = ctx.repo_store.join("items/data.txt");
    std::fs::remove_file(&file_path).unwrap(); // remove symlink
    std::fs::copy(&store_path, &file_path).unwrap(); // restore regular file

    // A second add on the regular file must fail with AlreadyManaged because
    // the manifest still contains the entry.
    let err = ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap_err();
    assert!(
        matches!(err, shelfbox_core::error::AppError::AlreadyManaged { .. }),
        "expected AlreadyManaged, got: {err}"
    );
}

#[test]
fn add_path_outside_repo_returns_error() {
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let outside_file = store_dir.path().join("outside.txt");
    std::fs::write(&outside_file, "outside").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    let err = ops::add::add_report(&mut ctx, &outside_file, false, &link, &ignore).unwrap_err();
    assert!(
        matches!(err, shelfbox_core::error::AppError::PathOutsideRepo { .. }),
        "expected PathOutsideRepo, got: {err}"
    );
}

#[test]
fn restore_regular_file_returns_destination_exists_error() {
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("plain.txt");
    std::fs::write(&file_path, "plain").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    // A regular file (not a symlink) must return RestoreDestinationExists, not
    // NotManagedLink, so the user gets a precise error and a helpful hint.
    let err = ops::restore::restore(&mut ctx, &file_path, false, false, false, &link, &ignore)
        .unwrap_err();
    assert!(
        matches!(
            err,
            shelfbox_core::error::AppError::RestoreDestinationExists { .. }
        ),
        "expected RestoreDestinationExists, got: {err}"
    );
}

#[test]
fn restore_nonexistent_path_returns_not_managed_link_error() {
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    // A path that does not exist at all.
    let file_path = repo_dir.path().join("does_not_exist.txt");

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    let err = ops::restore::restore(&mut ctx, &file_path, false, false, false, &link, &ignore)
        .unwrap_err();
    assert!(
        matches!(err, shelfbox_core::error::AppError::NotManagedLink { .. }),
        "expected NotManagedLink, got: {err}"
    );
}

// ── keep_ignore ---

#[test]
fn restore_keep_ignore_preserves_exclude_entry() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("env.sh");
    std::fs::write(&file_path, "export SECRET=1").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    assert!(ignore.has_entry(repo_dir.path(), "env.sh").unwrap());

    // Restore with keep_ignore=true.
    ops::restore::restore(&mut ctx, &file_path, false, true, false, &link, &ignore).unwrap();

    // Entry must still be present.
    assert!(
        ignore.has_entry(repo_dir.path(), "env.sh").unwrap(),
        "keep_ignore=true must preserve the exclude entry"
    );
}

#[test]
fn restore_keep_store_leaves_symlink_and_store_item() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("keep.txt");
    std::fs::write(&file_path, "keep me").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    let store_path = ctx.repo_store.join("items/keep.txt");
    assert!(store_path.exists(), "store item must exist after add");

    // Restore with keep_store=true: item transitions to Detached state.
    // The manifest entry is retained for ownership tracking; symlink and
    // store item remain intact.
    ops::restore::restore(&mut ctx, &file_path, false, false, true, &link, &ignore).unwrap();

    // Item must still be in the manifest, but in Detached state.
    assert_eq!(
        ctx.manifest.items.len(),
        1,
        "manifest must retain the detached item"
    );
    assert_eq!(
        ctx.manifest.items[0].ownership_state,
        store::manifest::OwnershipState::Detached,
        "item must be in Detached state after keep_store restore"
    );
    // Store item must still exist.
    assert!(
        store_path.exists(),
        "store item must still exist after keep_store restore"
    );
    // Symlink must still be in place.
    assert!(
        file_path
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "symlink must still be in place after keep_store restore"
    );
}

#[test]
fn restore_keep_store_dry_run_reports_plan_and_makes_no_changes() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("keep-dry-run.txt");
    std::fs::write(&file_path, "keep me").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    let repo_before = common::snapshot_tree(repo_dir.path());
    let store_before = common::snapshot_tree(store_dir.path());

    let report =
        ops::restore::restore(&mut ctx, &file_path, true, false, true, &link, &ignore).unwrap();

    assert!(report.dry_run);
    assert_eq!(report.plan.path, "keep-dry-run.txt");
    assert_eq!(report.plan.action, ItemRestoreAction::DetachKeepStore);
    assert!(!report.plan.keep_ignore);
    assert_eq!(
        ctx.manifest.items[0].ownership_state,
        store::manifest::OwnershipState::Attached,
        "dry-run must not update in-memory ownership state"
    );
    assert_eq!(common::snapshot_tree(repo_dir.path()), repo_before);
    assert_eq!(common::snapshot_tree(store_dir.path()), store_before);
}

#[test]
fn relink_dry_run_makes_no_changes() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("detached.txt");
    std::fs::write(&file_path, "keep me detached").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    ops::restore::restore(&mut ctx, &file_path, false, false, true, &link, &ignore).unwrap();
    assert_eq!(
        ctx.manifest.items[0].ownership_state,
        store::manifest::OwnershipState::Detached
    );

    let repo_before = common::snapshot_tree(repo_dir.path());
    let store_before = common::snapshot_tree(store_dir.path());

    let outcome = ops::relink::relink_report(&mut ctx, &file_path, true, &link).unwrap();

    assert_eq!(outcome.outcome, ops::relink::RelinkOutcome::WouldRelink);
    assert_eq!(
        ctx.manifest.items[0].ownership_state,
        store::manifest::OwnershipState::Detached,
        "dry-run must not update in-memory ownership state"
    );
    assert_eq!(common::snapshot_tree(repo_dir.path()), repo_before);
    assert_eq!(common::snapshot_tree(store_dir.path()), store_before);
}

// ── doctor ────────────────────────────────────────────────────────────────────

#[test]
fn doctor_finds_orphan_store_item() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("orphan_test.txt");
    std::fs::write(&file_path, "test").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    // Shelve the file, then manually inject an orphan file into the store.
    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    let orphan_path = ctx.items_dir().join("orphan_injected.txt");
    std::fs::write(&orphan_path, "orphan").unwrap();

    let report = ops::integrity::check(&ctx, &link, &ignore).unwrap();

    assert_eq!(report.items.len(), 1);
    assert!(report.items[0].ok, "managed item must be reported as ok");
    assert_eq!(report.orphan_store_items.len(), 1);
    assert_eq!(report.orphan_store_items[0], "orphan_injected.txt");
}

#[test]
fn doctor_empty_repo_is_clean() {
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    let report = ops::integrity::check(&ctx, &link, &ignore).unwrap();

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
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    // Create, stage and commit a file so it is tracked by Git.
    let file_path = repo_dir.path().join("tracked.txt");
    std::fs::write(&file_path, "contents").unwrap();
    common::run_git(repo_dir.path(), &["add", "tracked.txt"]);
    common::run_git(repo_dir.path(), &["commit", "-m", "add tracked file"]);

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    let err = ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap_err();
    assert!(
        matches!(err, shelfbox_core::error::AppError::PathIsTracked { .. }),
        "expected PathIsTracked, got: {err}"
    );
}

#[test]
fn add_git_dir_path_returns_error() {
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    // Target a file inside .git/ (e.g. .git/config, which always exists).
    let git_config = repo_dir.path().join(".git").join("config");

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    let err = ops::add::add_report(&mut ctx, &git_config, false, &link, &ignore).unwrap_err();
    assert!(
        matches!(err, shelfbox_core::error::AppError::PathInsideGitDir { .. }),
        "expected PathInsideGitDir, got: {err}"
    );
}

#[test]
fn add_existing_symlink_returns_error() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    // Create a symlink that is not managed by shelfbox.
    let target = repo_dir.path().join("target_file.txt");
    std::fs::write(&target, "target").unwrap();
    let link_path = repo_dir.path().join("my_link");
    common::create_file_symlink(&target, &link_path);

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    let err = ops::add::add_report(&mut ctx, &link_path, false, &link, &ignore).unwrap_err();
    assert!(
        matches!(err, shelfbox_core::error::AppError::PathIsSymlink { .. }),
        "expected PathIsSymlink, got: {err}"
    );
}

// ── doctor status checks ──────────────────────────────────────────────────────

#[test]
fn doctor_reports_error_for_dangling_symlink() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("secrets.txt");
    std::fs::write(&file_path, "secret").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    // Remove the store-side file to create a dangling symlink.
    let store_path = ctx.repo_store.join("items/secrets.txt");
    std::fs::remove_file(&store_path).unwrap();

    let report = ops::integrity::check(&ctx, &link, &ignore).unwrap();

    assert_eq!(report.items.len(), 1);
    let s = &report.items[0];
    assert!(s.link_exists, "symlink node still present at repo side");
    assert!(!s.store_exists, "store item was removed; should be false");
    assert!(!s.ok, "item with missing store should not be ok");
}

#[test]
fn doctor_reports_warn_for_missing_exclude_entry() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("private.txt");
    std::fs::write(&file_path, "private").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    // Manually remove the exclude entry to simulate a WARN condition.
    ignore
        .remove_entries(repo_dir.path(), &["private.txt"])
        .unwrap();

    let report = ops::integrity::check(&ctx, &link, &ignore).unwrap();

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
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("secret.env");
    std::fs::write(&file_path, "TOKEN=abc").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    assert!(file_path
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());

    // Simulate the symlink being removed (e.g. by `rm`).
    std::fs::remove_file(&file_path).unwrap();
    assert!(!file_path.exists(), "symlink must be gone before repair");

    let outcome = ops::repair::repair_report(&ctx, &file_path, &link, false, false).unwrap();
    assert_eq!(outcome.outcome, ops::repair::RepairOutcome::LinkRecreated);

    // Symlink must be back and readable.
    assert!(file_path
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "TOKEN=abc");
}

#[test]
fn repair_rejects_wrong_target_symlink_without_force() {
    if !require_symlink_support() {
        return;
    }
    // A symlink that points outside the managed store must NOT be silently
    // overwritten.  repair() must return RepairSymlinkTargetMismatch so the
    // user can investigate before running with --force.
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("cfg.toml");
    std::fs::write(&file_path, "[db]").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    // Replace managed symlink with one pointing elsewhere (hand-modified).
    std::fs::remove_file(&file_path).unwrap();
    let bogus_target = repo_dir.path().join("missing-target-for-repair-test-1");
    common::create_file_symlink(&bogus_target, &file_path);
    assert!(
        !link.is_managed_link(&file_path, &ctx.config.store),
        "symlink must not be managed before the test"
    );

    // Without --force, repair must refuse.
    let result = ops::repair::repair_report(&ctx, &file_path, &link, false, false);
    assert!(
        matches!(
            result,
            Err(shelfbox_core::error::AppError::RepairSymlinkTargetMismatch { .. })
        ),
        "expected RepairSymlinkTargetMismatch, got: {result:?}"
    );

    // The wrong-target symlink must be untouched.
    assert!(
        !link.is_managed_link(&file_path, &ctx.config.store),
        "wrong-target symlink must not have been changed"
    );
}

#[test]
fn repair_force_relinks_wrong_target_symlink() {
    if !require_symlink_support() {
        return;
    }
    // With --force, repair must overwrite a wrong-target symlink and restore
    // the correct link to the managed store item.
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("cfg.toml");
    std::fs::write(&file_path, "[db]").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    // Replace managed symlink with one pointing elsewhere.
    std::fs::remove_file(&file_path).unwrap();
    let bogus_target = repo_dir.path().join("missing-target-for-repair-test-2");
    common::create_file_symlink(&bogus_target, &file_path);

    let outcome = ops::repair::repair_report(&ctx, &file_path, &link, false, true).unwrap();
    assert_eq!(outcome.outcome, ops::repair::RepairOutcome::LinkRecreated);

    assert!(link.is_managed_link(&file_path, &ctx.config.store));
    assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "[db]");
}

#[test]
fn repair_already_healthy_returns_no_op() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("healthy.txt");
    std::fs::write(&file_path, "ok").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    let outcome = ops::repair::repair_report(&ctx, &file_path, &link, false, false).unwrap();
    assert_eq!(outcome.outcome, ops::repair::RepairOutcome::AlreadyHealthy);
}

#[test]
fn repair_returns_store_missing_when_store_item_gone() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("secrets.txt");
    std::fs::write(&file_path, "secret").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    // Delete the store-side copy to simulate data loss.
    let store_item = ctx.repo_store.join("items/secrets.txt");
    std::fs::remove_file(&store_item).unwrap();

    let outcome = ops::repair::repair_report(&ctx, &file_path, &link, false, false).unwrap();
    assert_eq!(outcome.outcome, ops::repair::RepairOutcome::StoreMissing);

    // The (now dangling) symlink must be left untouched.
    assert!(
        file_path.symlink_metadata().is_ok(),
        "repair must not remove the dangling symlink"
    );
}

#[test]
fn repair_returns_not_managed_for_unknown_path() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let unmanaged = repo_dir.path().join("unmanaged.txt");
    std::fs::write(&unmanaged, "not shelved").unwrap();

    let ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;

    let outcome = ops::repair::repair_report(&ctx, &unmanaged, &link, false, false).unwrap();
    assert_eq!(outcome.outcome, ops::repair::RepairOutcome::NotManaged);
}

#[test]
fn repair_refuses_to_overwrite_regular_file() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("data.txt");
    std::fs::write(&file_path, "original").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();

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

    let result = ops::repair::repair_report(&ctx, &file_path, &link, false, false);
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
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("dryrun.txt");
    std::fs::write(&file_path, "contents").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    std::fs::remove_file(&file_path).unwrap();
    let repo_before = common::snapshot_tree(repo_dir.path());
    let store_before = common::snapshot_tree(store_dir.path());

    let outcome = ops::repair::repair_report(&ctx, &file_path, &link, true, false).unwrap();
    assert_eq!(outcome.outcome, ops::repair::RepairOutcome::LinkRecreated);

    // Symlink must NOT have been recreated in dry-run mode.
    assert!(!file_path.exists(), "dry-run must not recreate the symlink");
    assert_eq!(common::snapshot_tree(repo_dir.path()), repo_before);
    assert_eq!(common::snapshot_tree(store_dir.path()), store_before);
}

#[test]
fn repo_repair_recreates_broken_symlinks() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("repo-secret.env");
    std::fs::write(&file_path, "TOKEN=repo").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    std::fs::remove_file(&file_path).unwrap();
    assert!(file_path.symlink_metadata().is_err());

    let report = ops::repair::repair_repo(&mut ctx, &link, false, false).unwrap();

    assert_eq!(report.symlinks_repaired, 1);
    assert_eq!(report.symlinks_already_healthy, 0);
    assert!(report.symlinks_failed.is_empty());
    assert!(link.is_managed_link(&file_path, &ctx.config.store));
    assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "TOKEN=repo");
}

#[test]
fn repo_repair_reports_healthy_symlinks_without_relinking() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("healthy-repo.txt");
    std::fs::write(&file_path, "ok").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    let target_before = link.read_target(&file_path).unwrap();
    let report = ops::repair::repair_repo(&mut ctx, &link, false, false).unwrap();
    let target_after = link.read_target(&file_path).unwrap();

    assert_eq!(report.symlinks_repaired, 0);
    assert_eq!(report.symlinks_already_healthy, 1);
    assert!(report.symlinks_failed.is_empty());
    assert_eq!(target_after, target_before);
}

#[test]
fn repo_repair_reports_missing_store_file_as_nonfatal_failure() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("lost-repo.txt");
    std::fs::write(&file_path, "lost").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    std::fs::remove_file(ctx.repo_store.join("items/lost-repo.txt")).unwrap();

    let report = ops::repair::repair_repo(&mut ctx, &link, false, false).unwrap();

    assert_eq!(report.symlinks_repaired, 0);
    assert_eq!(report.symlinks_already_healthy, 0);
    assert_eq!(report.symlinks_failed.len(), 1);
    assert!(!report.hints_updated);
    assert_eq!(report.symlinks_failed[0].0, "lost-repo.txt");
    assert!(report.symlinks_failed[0].1.contains("store item missing"));
    assert!(file_path.symlink_metadata().is_ok());
    let manifest = store::manifest::load(&ctx.repo_store).unwrap();
    assert_eq!(manifest.identity_hints.last_attached_at, None);
}

#[test]
fn repo_repair_updates_index_and_identity_hints() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("portable.txt");
    std::fs::write(&file_path, "portable").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    common::run_git(
        repo_dir.path(),
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:example/portable.git",
        ],
    );

    let mut idx = store::index::load(store_dir.path()).unwrap();
    let mut entry = idx.get(&ctx.repo_id).unwrap().clone();
    entry.git_dir = None;
    entry.git_common_dir = None;
    idx.upsert(&ctx.repo_id, entry);
    store::index::save(store_dir.path(), &idx).unwrap();

    let report = ops::repair::repair_repo(&mut ctx, &link, false, false).unwrap();
    let current = context::current_git_context(repo_dir.path()).unwrap();
    let idx = store::index::load(store_dir.path()).unwrap();
    let entry = idx.get(&ctx.repo_id).unwrap();
    let manifest = store::manifest::load(&ctx.repo_store).unwrap();

    assert!(report.index_updated);
    assert!(report.hints_updated);
    assert_eq!(entry.root.as_deref(), Some(current.repo_root.as_path()));
    assert_eq!(entry.git_dir.as_deref(), Some(current.git_dir.as_path()));
    assert_eq!(
        entry.git_common_dir.as_deref(),
        Some(current.git_common_dir.as_path())
    );
    assert!(manifest
        .identity_hints
        .remote_hints
        .contains(&"github.com/example/portable".to_string()));
    assert!(manifest.identity_hints.last_attached_at.is_some());
}

#[test]
fn repo_repair_requires_existing_repoid_without_creating_one() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("unassociated.txt");
    std::fs::write(&file_path, "data").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    let mut idx = store::index::load(store_dir.path()).unwrap();
    assert!(idx.remove(&ctx.repo_id));
    store::index::save(store_dir.path(), &idx).unwrap();

    let result = ops::repair::repair_repo(&mut ctx, &link, false, false);
    let idx_after = store::index::load(store_dir.path()).unwrap();

    assert!(result.is_err());
    assert_eq!(idx_after.iter().count(), 0);
}

#[test]
fn repo_repair_dry_run_makes_no_file_writes() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("dry-repo.txt");
    std::fs::write(&file_path, "dry").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    common::run_git(
        repo_dir.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/dry.git",
        ],
    );
    std::fs::remove_file(&file_path).unwrap();
    ignore
        .remove_entries(repo_dir.path(), &["dry-repo.txt"])
        .unwrap();

    let mut idx = store::index::load(store_dir.path()).unwrap();
    let mut entry = idx.get(&ctx.repo_id).unwrap().clone();
    entry.git_dir = None;
    entry.git_common_dir = None;
    idx.upsert(&ctx.repo_id, entry);
    store::index::save(store_dir.path(), &idx).unwrap();

    let manifest_path = store::manifest::manifest_path(&ctx.repo_store);
    let index_path = store::index::index_path(store_dir.path());
    let exclude_path = crate::git::exclude::exclude_file_path(repo_dir.path()).unwrap();
    let manifest_before = std::fs::read_to_string(&manifest_path).unwrap();
    let index_before = std::fs::read_to_string(&index_path).unwrap();
    let exclude_before = std::fs::read_to_string(&exclude_path).unwrap();
    let repo_before = common::snapshot_tree(repo_dir.path());
    let store_before = common::snapshot_tree(store_dir.path());

    let report = ops::repair::repair_repo(&mut ctx, &link, true, false).unwrap();

    assert_eq!(report.symlinks_repaired, 1);
    assert!(report.exclude_updated);
    assert!(report.index_updated);
    assert!(report.hints_updated);
    assert_eq!(report.plan.exclude_paths, vec!["dry-repo.txt"]);
    assert_eq!(report.plan.symlink_actions.len(), 1);
    assert!(matches!(
        &report.plan.symlink_actions[0],
        RepoRepairSymlinkAction::Recreate { path, .. } if path == "dry-repo.txt"
    ));
    assert!(file_path.symlink_metadata().is_err());
    assert_eq!(
        std::fs::read_to_string(&manifest_path).unwrap(),
        manifest_before
    );
    assert_eq!(std::fs::read_to_string(&index_path).unwrap(), index_before);
    assert_eq!(
        std::fs::read_to_string(&exclude_path).unwrap(),
        exclude_before
    );
    assert_eq!(common::snapshot_tree(repo_dir.path()), repo_before);
    assert_eq!(common::snapshot_tree(store_dir.path()), store_before);
}

// ── doctor --fix ──────────────────────────────────────────────────────────────

#[test]
fn doctor_fix_repairs_missing_exclude_entry() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("fix_exclude.env");
    std::fs::write(&file_path, "SECRET=1").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    // Manually remove the exclude entry to simulate the broken state.
    let exclude_path = repo_dir.path().join(".git/info/exclude");
    let content = std::fs::read_to_string(&exclude_path).unwrap();
    let stripped = content
        .lines()
        .filter(|l| !l.contains("fix_exclude.env"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&exclude_path, stripped).unwrap();
    assert!(
        !ignore
            .has_entry(repo_dir.path(), "fix_exclude.env")
            .unwrap(),
        "exclude entry must be absent before fix"
    );

    let report = ops::integrity::fix(&mut ctx, &link, &ignore, false, false).unwrap();

    // At least one Fixed action must be present.
    assert!(
        report
            .actions
            .iter()
            .any(|a| matches!(a, FixResult::Fixed(_))),
        "expected a Fixed action for exclude repair"
    );

    // The exclude entry must be restored.
    assert!(
        ignore
            .has_entry(repo_dir.path(), "fix_exclude.env")
            .unwrap(),
        "exclude entry must be present after fix"
    );
}

#[test]
fn doctor_fix_repairs_missing_symlink() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("fix_link.txt");
    std::fs::write(&file_path, "data").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    // Remove the symlink to simulate the broken state.
    std::fs::remove_file(&file_path).unwrap();

    let report = ops::integrity::fix(&mut ctx, &link, &ignore, false, false).unwrap();

    assert!(
        report
            .actions
            .iter()
            .any(|a| matches!(a, FixResult::Fixed(_))),
        "expected a Fixed action for symlink recreation"
    );
    assert!(
        file_path
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "symlink must be restored after fix"
    );
    assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "data");
}

#[test]
fn doctor_fix_records_cannot_fix_for_store_missing() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("lost.txt");
    std::fs::write(&file_path, "lost data").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    // Delete the store-side copy to simulate data loss.
    std::fs::remove_file(ctx.repo_store.join("items/lost.txt")).unwrap();

    let report = ops::integrity::fix(&mut ctx, &link, &ignore, false, false).unwrap();

    assert!(
        report
            .actions
            .iter()
            .any(|a| matches!(a, FixResult::CannotFix(_))),
        "expected CannotFix for store_missing"
    );
    assert!(
        !report.data_loss_warnings.is_empty(),
        "data_loss_warnings must be populated"
    );
}

#[test]
fn doctor_fix_true_orphan_is_reported_without_deletion() {
    if !require_symlink_support() {
        return;
    }
    // A store item with no repo-side symlink is unclassified data. Conservative
    // GC must leave it alone unless a manifest marks it `orphaned`.
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    // Inject a bare orphan: store item exists but no symlink in the repo.
    let orphan_path = ctx.items_dir().join("bare_orphan.txt");
    std::fs::create_dir_all(ctx.items_dir()).unwrap();
    std::fs::write(&orphan_path, "orphan").unwrap();

    let report = ops::integrity::fix(&mut ctx, &link, &ignore, false, false).unwrap();

    assert!(
        report
            .actions
            .iter()
            .any(|a| matches!(a, FixResult::Skipped(_))),
        "expected Skipped for unclassified store item"
    );
    // True orphan must NOT be absorbed into the manifest.
    assert!(
        !ctx.manifest.contains("bare_orphan.txt"),
        "true orphan must not be added to manifest"
    );
    // The store file must remain untouched.
    assert!(
        orphan_path.exists(),
        "unclassified store file must not be deleted"
    );
}

#[test]
fn doctor_fix_true_orphan_not_deleted_with_yes() {
    if !require_symlink_support() {
        return;
    }
    // Even with --yes, a store item with no repo-side symlink is not deleted
    // unless a manifest explicitly marks it `orphaned` for store gc.
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    let orphan_path = ctx.items_dir().join("bare_orphan_yes.txt");
    std::fs::create_dir_all(ctx.items_dir()).unwrap();
    std::fs::write(&orphan_path, "orphan").unwrap();

    let report = ops::integrity::fix(&mut ctx, &link, &ignore, true, false).unwrap();

    assert!(
        report
            .actions
            .iter()
            .any(|a| matches!(a, FixResult::Skipped(_))),
        "expected Skipped for unclassified store item"
    );
    assert!(
        orphan_path.exists(),
        "unclassified store file must not be deleted with --yes"
    );
}

#[test]
fn doctor_fix_dry_run_makes_no_changes() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("dryrun_fix.txt");
    std::fs::write(&file_path, "contents").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    // Remove both the symlink and the exclude entry to create a dirty state.
    std::fs::remove_file(&file_path).unwrap();
    let exclude_path = repo_dir.path().join(".git/info/exclude");
    let content = std::fs::read_to_string(&exclude_path).unwrap();
    let stripped = content
        .lines()
        .filter(|l| !l.contains("dryrun_fix.txt"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&exclude_path, stripped).unwrap();
    let repo_before = common::snapshot_tree(repo_dir.path());
    let store_before = common::snapshot_tree(store_dir.path());

    let report = ops::integrity::fix(&mut ctx, &link, &ignore, false, true).unwrap();

    // dry-run must report what it would do.
    assert!(
        report
            .actions
            .iter()
            .any(|a| matches!(a, FixResult::Fixed(_))),
        "dry-run should still report planned Fixed actions"
    );

    // Filesystem must be unchanged.
    assert!(!file_path.exists(), "dry-run must not recreate the symlink");
    assert!(
        !ignore.has_entry(repo_dir.path(), "dryrun_fix.txt").unwrap(),
        "dry-run must not restore the exclude entry"
    );
    assert_eq!(common::snapshot_tree(repo_dir.path()), repo_before);
    assert_eq!(common::snapshot_tree(store_dir.path()), store_before);
}

// ── doctor --fix: manifest rebuild (Phase 3) ──────────────────────────────────

#[test]
fn doctor_fix_rebuilds_manifest_when_missing() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    // Add two files so the store has content.
    for name in &["rebuild_a.txt", "rebuild_b.txt"] {
        let file_path = repo_dir.path().join(name);
        std::fs::write(&file_path, name).unwrap();
        let mut ctx =
            context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
        let link = DefaultLinkStrategy;
        let ignore = GitInfoExclude;
        ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    }

    // Delete the manifest to simulate complete manifest loss.
    let manifest_path = {
        let ctx_check =
            context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
        let p = ctx_check.repo_store.join("manifest.json");
        std::fs::remove_file(&p).unwrap();
        p
    }; // write lock released here

    // Rebuild via doctor --fix --yes (rebuild requires explicit confirmation).
    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    assert_eq!(
        ctx.manifest.items.len(),
        0,
        "manifest must be empty after deletion"
    );
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    let report = ops::integrity::fix(&mut ctx, &link, &ignore, true, false).unwrap();

    // A Fixed action must be present for the rebuild.
    assert!(
        report
            .actions
            .iter()
            .any(|a| matches!(a, FixResult::Fixed(_))),
        "expected Fixed action for manifest rebuild"
    );

    // Both items must now be in the manifest.
    assert_eq!(
        ctx.manifest.items.len(),
        2,
        "manifest must contain both items after rebuild"
    );
    assert!(ctx.manifest.contains("rebuild_a.txt"));
    assert!(ctx.manifest.contains("rebuild_b.txt"));

    // The manifest file must be persisted.
    assert!(
        manifest_path.exists(),
        "manifest file must exist after rebuild"
    );
}

#[test]
fn doctor_fix_rebuilt_manifest_produces_healthy_status() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("status_after_rebuild.txt");
    std::fs::write(&file_path, "contents").unwrap();

    {
        let mut ctx =
            context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
        let link = DefaultLinkStrategy;
        let ignore = GitInfoExclude;
        ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    }

    // Delete only the manifest; the symlink at the repo path remains intact.
    // This simulates manifest loss while the shelved item is still accessible
    // via its symlink — the canonical scenario for manifest reconstruction.
    {
        let ctx_check =
            context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
        std::fs::remove_file(ctx_check.repo_store.join("manifest.json")).unwrap();
    } // write lock released here

    // doctor --fix --yes should rebuild the manifest (symlink exists → rebuild candidate).
    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    ops::integrity::fix(&mut ctx, &link, &ignore, true, false).unwrap();

    // Status must be healthy.
    let statuses = ops::status::status(&ctx, &link, &ignore).unwrap();
    assert_eq!(statuses.len(), 1);
    assert!(
        statuses[0].link_valid,
        "symlink must be valid after rebuild"
    );
    assert!(statuses[0].store_exists, "store item must exist");
}

#[test]
fn doctor_fix_rebuilds_only_missing_items_when_partial() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    // Add two files normally.
    for name in &["partial_a.txt", "partial_b.txt"] {
        let file_path = repo_dir.path().join(name);
        std::fs::write(&file_path, name).unwrap();
        let mut ctx =
            context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
        let link = DefaultLinkStrategy;
        let ignore = GitInfoExclude;
        ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    }

    // Remove only partial_b from the manifest by rewriting it with just partial_a.
    {
        let mut ctx =
            context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
        ctx.manifest.remove("partial_b.txt");
        shelfbox_core::store::manifest::save(&ctx.repo_store, &ctx.manifest).unwrap();
    } // write lock released here

    // doctor --fix --yes should add only partial_b (partial_a is already there).
    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    assert_eq!(
        ctx.manifest.items.len(),
        1,
        "only partial_a should be in manifest before fix"
    );
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    ops::integrity::fix(&mut ctx, &link, &ignore, true, false).unwrap();

    assert_eq!(
        ctx.manifest.items.len(),
        2,
        "both items must be present after fix"
    );
    assert!(
        ctx.manifest.contains("partial_a.txt"),
        "partial_a must still be present"
    );
    assert!(
        ctx.manifest.contains("partial_b.txt"),
        "partial_b must have been re-added"
    );
}

#[test]
fn doctor_fix_mixed_rebuild_candidate_and_true_orphan() {
    if !require_symlink_support() {
        return;
    }
    // Scenario: one store item has a valid symlink (rebuild candidate) and
    // another has no symlink (unclassified store data). Without --yes,
    // doctor --fix must report the rebuild candidate as needing confirmation,
    // skip the unclassified item, and not modify the manifest or delete data.
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    // Shelve a file normally (creates symlink + manifest + exclude).
    let file_path = repo_dir.path().join("managed_mixed.txt");
    std::fs::write(&file_path, "managed").unwrap();
    {
        let mut ctx =
            context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
        let link = DefaultLinkStrategy;
        let ignore = GitInfoExclude;
        ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    }

    // Simulate manifest loss (symlink remains).
    {
        let ctx_check =
            context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
        std::fs::remove_file(ctx_check.repo_store.join("manifest.json")).unwrap();
    } // write lock released here

    // Inject a bare orphan (no symlink at repo path).
    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let orphan_path = ctx.items_dir().join("bare_mixed_orphan.txt");
    std::fs::write(&orphan_path, "orphan").unwrap();

    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    let report = ops::integrity::fix(&mut ctx, &link, &ignore, false, false).unwrap();

    // Neither item must be absorbed into the manifest without --yes.
    assert!(
        !ctx.manifest.contains("managed_mixed.txt"),
        "rebuild candidate must not be absorbed without --yes"
    );
    assert!(
        !ctx.manifest.contains("bare_mixed_orphan.txt"),
        "true orphan must not be added to manifest"
    );
    // The rebuild candidate needs confirmation; the unclassified store item is skipped.
    let confirmation_count = report
        .actions
        .iter()
        .filter(|a| matches!(a, FixResult::NeedsConfirmation(_)))
        .count();
    let skipped_count = report
        .actions
        .iter()
        .filter(|a| matches!(a, FixResult::Skipped(_)))
        .count();
    assert!(
        confirmation_count >= 1,
        "expected NeedsConfirmation for rebuild candidate, got {confirmation_count}"
    );
    assert!(
        skipped_count >= 1,
        "expected Skipped for unclassified store item, got {skipped_count}"
    );
    // Unclassified store file must remain.
    assert!(
        orphan_path.exists(),
        "unclassified store file must not be deleted"
    );
}

#[test]
fn doctor_fix_rebuild_dry_run_does_not_persist() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("rebuild_dry.txt");
    std::fs::write(&file_path, "data").unwrap();

    {
        let mut ctx =
            context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
        let link = DefaultLinkStrategy;
        let ignore = GitInfoExclude;
        ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    }

    // Delete manifest to force rebuild path.
    let manifest_path = {
        let ctx_check =
            context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
        let p = ctx_check.repo_store.join("manifest.json");
        std::fs::remove_file(&p).unwrap();
        p
    }; // write lock released here

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    let repo_before = common::snapshot_tree(repo_dir.path());
    let store_before = common::snapshot_tree(store_dir.path());
    // yes=true so rebuild is attempted; dry_run=true so nothing is written.
    let report = ops::integrity::fix(&mut ctx, &link, &ignore, true, true).unwrap();

    // Report must still mention the planned action.
    assert!(
        report
            .actions
            .iter()
            .any(|a| matches!(a, FixResult::Fixed(_))),
        "dry-run should report the planned rebuild"
    );

    // In-memory manifest should NOT be modified in dry-run.
    assert_eq!(
        ctx.manifest.items.len(),
        0,
        "dry-run must not modify the in-memory manifest"
    );

    // Manifest file must still be absent.
    assert!(
        !manifest_path.exists(),
        "dry-run must not write the manifest file"
    );
    assert_eq!(common::snapshot_tree(repo_dir.path()), repo_before);
    assert_eq!(common::snapshot_tree(store_dir.path()), store_before);
}

#[test]
fn doctor_fix_wrong_target_symlink_is_not_a_rebuild_candidate() {
    if !require_symlink_support() {
        return;
    }
    // A symlink at the expected repo-relative path that points to the WRONG
    // store location must NOT be absorbed as a rebuild candidate.  Only a
    // symlink whose target matches `<repo_store>/items/<path>` is valid.
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();
    let other_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("secret.txt");
    std::fs::write(&file_path, "data").unwrap();

    // Add the file normally so a store item exists.
    {
        let mut ctx =
            context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
        let link = DefaultLinkStrategy;
        let ignore = GitInfoExclude;
        ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();
    }

    // Delete manifest and replace the repo-side symlink with one that points
    // to a different location (simulating a stale symlink from a re-clone or
    // another tool).
    {
        let ctx_check =
            context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
        std::fs::remove_file(ctx_check.repo_store.join("manifest.json")).unwrap();
    }

    // Remove the correct symlink and create a wrong-target one.
    std::fs::remove_file(&file_path).unwrap();
    let decoy_target = other_dir.path().join("decoy.txt");
    std::fs::write(&decoy_target, "unrelated").unwrap();
    common::create_file_symlink(&decoy_target, &file_path);

    // doctor --fix --yes: the store item has no manifest entry AND no correct
    // symlink, so it must be treated as an orphan, not a rebuild candidate.
    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    ops::integrity::fix(&mut ctx, &link, &ignore, true, false).unwrap();

    // The item must NOT have been added to the manifest via rebuild.
    assert!(
        !ctx.manifest.contains("secret.txt"),
        "wrong-target symlink must not be absorbed as a rebuild candidate"
    );
}

// ── move_item ─────────────────────────────────────────────────────────────────

#[test]
fn move_item_renames_store_and_updates_symlink() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let old_path = repo_dir.path().join("old.txt");
    std::fs::write(&old_path, "file content").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &old_path, false, &link, &ignore).unwrap();
    assert!(old_path
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());

    // Move to a subdirectory to also test parent directory creation.
    let new_path = repo_dir.path().join("subdir/new.txt");
    ops::move_item::move_item(&mut ctx, &old_path, &new_path, false, &link, &ignore).unwrap();

    // Old symlink must be gone.
    assert!(!old_path.exists(), "old symlink must be removed");

    // New symlink must exist and be readable.
    assert!(
        new_path
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "new symlink must exist"
    );
    assert_eq!(std::fs::read_to_string(&new_path).unwrap(), "file content");

    // Store item must have moved.
    assert!(
        !ctx.repo_store.join("items/old.txt").exists(),
        "old store path must not exist"
    );
    assert!(
        ctx.repo_store.join("items/subdir/new.txt").exists(),
        "new store path must exist"
    );

    // Manifest must reflect the new path.
    assert!(
        !ctx.manifest.contains("old.txt"),
        "old path must be removed from manifest"
    );
    assert!(
        ctx.manifest.contains("subdir/new.txt"),
        "new path must be in manifest"
    );
    let item = ctx.manifest.get("subdir/new.txt").unwrap();
    assert_eq!(item.store_path, "items/subdir/new.txt");

    // Exclude must have the new entry and not the old one.
    assert!(
        !ignore.has_entry(repo_dir.path(), "old.txt").unwrap(),
        "old exclude entry must be removed"
    );
    assert!(
        ignore.has_entry(repo_dir.path(), "subdir/new.txt").unwrap(),
        "new exclude entry must be added"
    );
}

#[test]
fn move_item_rejects_when_destination_exists() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let old_path = repo_dir.path().join("source.txt");
    std::fs::write(&old_path, "content").unwrap();

    // A file already occupying the destination.
    let new_path = repo_dir.path().join("existing.txt");
    std::fs::write(&new_path, "already here").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &old_path, false, &link, &ignore).unwrap();

    let err = ops::move_item::move_item(&mut ctx, &old_path, &new_path, false, &link, &ignore)
        .unwrap_err();
    assert!(
        matches!(
            err,
            shelfbox_core::error::AppError::MoveDestinationExists { .. }
        ),
        "expected MoveDestinationExists, got: {err}"
    );

    // Original item must be untouched.
    assert!(ctx.manifest.contains("source.txt"));
    assert!(ctx.repo_store.join("items/source.txt").exists());
}

#[test]
fn move_item_rejects_when_new_path_already_managed() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_a = repo_dir.path().join("a.txt");
    let file_b = repo_dir.path().join("b.txt");
    std::fs::write(&file_a, "aaa").unwrap();
    std::fs::write(&file_b, "bbb").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &file_a, false, &link, &ignore).unwrap();
    ops::add::add_report(&mut ctx, &file_b, false, &link, &ignore).unwrap();

    // Attempt to move a.txt → b.txt where b.txt is already managed.
    let err =
        ops::move_item::move_item(&mut ctx, &file_a, &file_b, false, &link, &ignore).unwrap_err();
    assert!(
        matches!(err, shelfbox_core::error::AppError::AlreadyManaged { .. }),
        "expected AlreadyManaged, got: {err}"
    );

    // Both items must remain intact.
    assert!(ctx.manifest.contains("a.txt"));
    assert!(ctx.manifest.contains("b.txt"));
}

#[test]
fn move_item_rejects_when_symlink_mismatch() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let file_path = repo_dir.path().join("secret.txt");
    std::fs::write(&file_path, "secret").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &file_path, false, &link, &ignore).unwrap();

    // Replace the managed symlink with one pointing elsewhere.
    std::fs::remove_file(&file_path).unwrap();
    let bogus_target = repo_dir.path().join("missing-target-for-move-item-test");
    common::create_file_symlink(&bogus_target, &file_path);

    let new_path = repo_dir.path().join("secret_renamed.txt");
    let err = ops::move_item::move_item(&mut ctx, &file_path, &new_path, false, &link, &ignore)
        .unwrap_err();
    assert!(
        matches!(
            err,
            shelfbox_core::error::AppError::MoveSourceSymlinkMismatch { .. }
        ),
        "expected MoveSourceSymlinkMismatch, got: {err}"
    );

    // Store item must be untouched.
    assert!(ctx.repo_store.join("items/secret.txt").exists());
}

#[test]
fn move_item_dry_run_makes_no_changes() {
    if !require_symlink_support() {
        return;
    }
    let repo_dir = common::init_git_repo();
    let store_dir = TempDir::new().unwrap();

    let old_path = repo_dir.path().join("original.txt");
    std::fs::write(&old_path, "data").unwrap();

    let mut ctx = context::build_create_or_load(repo_dir.path(), Some(store_dir.path())).unwrap();
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    ops::add::add_report(&mut ctx, &old_path, false, &link, &ignore).unwrap();

    let new_path = repo_dir.path().join("renamed.txt");
    let repo_before = common::snapshot_tree(repo_dir.path());
    let store_before = common::snapshot_tree(store_dir.path());
    let report =
        ops::move_item::move_item(&mut ctx, &old_path, &new_path, true, &link, &ignore).unwrap();
    assert!(report.dry_run);
    assert!(report.warnings.is_empty());
    assert_eq!(report.plan.old_path, "original.txt");
    assert_eq!(report.plan.new_path, "renamed.txt");
    assert_eq!(
        report.plan.old_store_path,
        ctx.repo_store.join("items/original.txt")
    );
    assert_eq!(
        report.plan.new_store_path,
        ctx.repo_store.join("items/renamed.txt")
    );

    // Symlink at old path must still be present.
    assert!(
        old_path
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "dry-run must not remove old symlink"
    );
    // No symlink at new path.
    assert!(!new_path.exists(), "dry-run must not create new symlink");
    // Store item must remain at old location.
    assert!(ctx.repo_store.join("items/original.txt").exists());
    assert!(!ctx.repo_store.join("items/renamed.txt").exists());
    // Manifest must be unchanged.
    assert!(ctx.manifest.contains("original.txt"));
    assert!(!ctx.manifest.contains("renamed.txt"));
    assert_eq!(common::snapshot_tree(repo_dir.path()), repo_before);
    assert_eq!(common::snapshot_tree(store_dir.path()), store_before);
}
