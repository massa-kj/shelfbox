use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use serde::Serialize;

use super::status::{self, ItemStatus, ItemStatusV2, StatusOptions};
use crate::{
    context::RepoContext, error::Result, ignore::IgnoreBackend, link::LinkStrategy, store::index,
};

#[cfg(test)]
use super::repair::{self, RepairOutcome};
#[cfg(test)]
use crate::{context, store::manifest};
#[cfg(test)]
use ulid::Ulid;

/// Comprehensive health report produced by integrity checks.
///
/// This is a read-only operation.
#[derive(Debug, Serialize)]
pub struct IntegrityReport {
    /// Per-item health status (covers every item in the manifest).
    pub items: Vec<ItemStatus>,
    /// Store-side paths that exist on disk but are not referenced in the manifest.
    pub orphan_store_items: Vec<String>,
    /// `true` when the repo root recorded in the index matches `ctx.repo_root`.
    pub repo_root_matches_index: bool,
}

#[derive(Debug, Serialize)]
pub struct IntegrityReportV2 {
    /// Per-item schema-v2 health status (covers every item in the manifest).
    pub items: Vec<ItemStatusV2>,
    /// Store-side paths that exist on disk but are not referenced in the manifest.
    pub orphan_store_items: Vec<String>,
    /// `true` when the repo root recorded in the index matches `ctx.repo_root`.
    pub repo_root_matches_index: bool,
}

/// Runs all health checks and returns an [`IntegrityReport`].
pub fn check(
    ctx: &RepoContext,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<IntegrityReport> {
    let items = status::status(ctx, link, ignore)?;
    let orphan_store_items = collect_orphan_store_items(ctx);
    let repo_root_matches_index = check_repo_root_in_index(ctx)?;
    Ok(IntegrityReport {
        items,
        orphan_store_items,
        repo_root_matches_index,
    })
}

/// Runs all health checks and returns a schema-v2 [`IntegrityReportV2`].
pub fn check_v2(
    ctx: &RepoContext,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
    options: StatusOptions,
) -> Result<IntegrityReportV2> {
    let items = status::status_v2(ctx, link, ignore, options)?;
    let orphan_store_items = collect_orphan_store_items(ctx);
    let repo_root_matches_index = check_repo_root_in_index(ctx)?;
    Ok(IntegrityReportV2 {
        items,
        orphan_store_items,
        repo_root_matches_index,
    })
}

/// Checks whether the repo root stored in the global index matches
/// [`ctx.repo_root`].
///
/// A mismatch indicates that the repository was moved or cloned to a different
/// path on this machine since it was first shelved.
fn check_repo_root_in_index(ctx: &RepoContext) -> Result<bool> {
    let idx = index::load(&ctx.config.store)?;
    Ok(idx
        .get(&ctx.repo_id)
        .map(|e| e.root.as_deref() == Some(ctx.repo_root.as_path()))
        .unwrap_or(false))
}

/// Walks the `items/` subtree of the repo store and collects any path that is
/// not referenced by the manifest.
fn collect_orphan_store_items(ctx: &RepoContext) -> Vec<String> {
    let items_dir = ctx.items_dir();
    if !items_dir.exists() {
        return Vec::new();
    }

    // Absolute paths for every item currently owned by the manifest.
    let managed: HashSet<PathBuf> = ctx
        .manifest
        .items
        .iter()
        .map(|i| ctx.repo_store.join(&i.store_path))
        .collect();

    let mut orphans = Vec::new();
    walk_for_orphans(&items_dir, &items_dir, &managed, &mut orphans);
    orphans
}

/// Recursively walks `dir`, collecting entries that are neither in `managed`
/// nor an ancestor directory of a managed path.
fn walk_for_orphans(
    dir: &Path,
    items_root: &Path,
    managed: &HashSet<PathBuf>,
    orphans: &mut Vec<String>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();

        if managed.contains(&path) {
            // Known managed item; do not recurse into managed directories.
            continue;
        }

        // Is this an intermediate directory created to hold managed items?
        let is_ancestor = managed.iter().any(|m| m.starts_with(&path));
        if is_ancestor {
            if path.is_dir() {
                walk_for_orphans(&path, items_root, managed, orphans);
            }
        } else {
            // Neither managed nor a parent of a managed item → orphan.
            if let Ok(rel) = path.strip_prefix(items_root) {
                orphans.push(rel.to_string_lossy().into_owned());
            }
            // Report orphan directories at their root; do not recurse into them.
        }
    }
}

#[cfg(test)]
mod schema_v2_tests {
    use super::*;

    #[test]
    fn repo_status_v2_json_shape_matches_golden_fixture() {
        let report = IntegrityReportV2 {
            items: vec![status::healthy_symlink_status_v2_fixture()],
            orphan_store_items: Vec::new(),
            repo_root_matches_index: true,
        };
        let actual = format!("{}\n", serde_json::to_string_pretty(&report).unwrap());

        assert_eq!(
            actual,
            include_str!("../../tests/fixtures/repo-status-symlink-v2.json")
        );
    }
}

// ── fix mode ──────────────────────────────────────────────────────────────────

/// The outcome of a single test-only fix action.
#[cfg(test)]
#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "detail")]
pub enum FixResult {
    /// The issue was successfully resolved.
    Fixed(String),
    /// No fix was needed; the item was already healthy.
    Skipped(String),
    /// An error occurred while attempting the fix.  Other actions continued.
    Failed(String),
    /// The action is potentially destructive and requires `--yes` to proceed.
    NeedsConfirmation(String),
    /// The issue cannot be auto-repaired (e.g. store item missing / data loss).
    CannotFix(String),
}

/// Report produced by the test-only integrity fix helper.
#[cfg(test)]
#[derive(Debug, Serialize)]
pub struct IntegrityFixReport {
    /// Ordered log of every fix action attempted.
    pub actions: Vec<FixResult>,
    /// Items whose store-side file is missing; surfaced separately for easy
    /// inspection without iterating `actions`.
    pub data_loss_warnings: Vec<String>,
}

/// Diagnoses all known issues and applies safe automatic fixes.
///
/// **Safety levels:**
/// - Safe (no data loss possible): performed automatically.
/// - Potentially destructive unclassified data: reported but left untouched.
/// - Cannot fix (store item missing): recorded in `data_loss_warnings`.
///
/// Actions are applied in order, and failures are recorded but do not
/// abort remaining steps (best-effort).  The function is idempotent.
///
/// `dry_run = true` records what *would* happen without touching the
/// filesystem, index, or ignore backend.
#[cfg(test)]
pub fn fix(
    ctx: &mut RepoContext,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
    yes: bool,
    dry_run: bool,
) -> Result<IntegrityFixReport> {
    let mut actions: Vec<FixResult> = Vec::new();
    let mut data_loss_warnings: Vec<String> = Vec::new();

    // Order matters:
    // 1. Fix root first so subsequent ops see consistent paths.
    // 2. Rebuild manifest (only when yes=true; otherwise report candidates).
    // 3. Add missing exclude entries (uses updated manifest).
    // 4. Recreate symlinks (uses updated manifest).
    // 5. Handle any remaining orphans.
    fix_root_mismatch(ctx, &mut actions, dry_run)?;
    rebuild_manifest_from_store(ctx, &mut actions, yes, dry_run)?;
    fix_exclude_entries(ctx, ignore, &mut actions, dry_run)?;
    fix_symlinks(ctx, link, &mut actions, &mut data_loss_warnings, dry_run);
    handle_orphans(ctx, yes, &mut actions, dry_run);

    Ok(IntegrityFixReport {
        actions,
        data_loss_warnings,
    })
}

/// Scans the `items/` directory for files not recorded in the manifest and
/// adds them as new manifest entries.
///
/// This recovers from manifest loss or partial inconsistency.  The
/// `store_path` layout (`items/{repo_relative_path}`) makes the reverse
/// mapping deterministic — no guessing required.
///
/// Metadata that cannot be recovered (`created_at`, `updated_at`) is set to
/// the current time.
///
/// Already-managed items are skipped (`manifest.contains()` check) so the
/// function is safe to call even when the manifest is partially intact.
///
/// When `yes` is `false`, rebuild candidates are reported as
/// [`FixResult::NeedsConfirmation`] and the manifest is not modified.
#[cfg(test)]
fn rebuild_manifest_from_store(
    ctx: &mut RepoContext,
    actions: &mut Vec<FixResult>,
    yes: bool,
    dry_run: bool,
) -> Result<()> {
    // Only absorb store items that have a corresponding symlink at the expected
    // repo path AND whose symlink target matches the expected store path.
    //
    // Two conditions must both be true for a "rebuild candidate":
    //   1. A symlink exists at the repo-relative path.
    //   2. The symlink target is exactly the store item's canonical location.
    //
    // Condition (2) guards against a stale symlink that points to an old store
    // (e.g. after a re-clone) or an unrelated symlink that happens to share the
    // same repo-relative path.  Only absorbing items whose target provably
    // points back to THIS store keeps the operation safe.
    //
    // Store files with NO matching repo symlink (or a symlink pointing
    // elsewhere) are genuine orphans and are left for `handle_orphans`.
    let to_add: Vec<String> = collect_orphan_store_items(ctx)
        .into_iter()
        .filter(|o| !ctx.manifest.contains(o))
        .filter(|o| {
            // Require a symlink whose target is the expected store path.
            let abs_repo_path = ctx.repo_root.join(o);
            let expected_store_path = ctx.items_dir().join(o);
            std::fs::read_link(&abs_repo_path)
                .map(|target| {
                    // Resolve relative symlinks before comparing.
                    let abs_target = if target.is_absolute() {
                        target
                    } else {
                        abs_repo_path
                            .parent()
                            .map(|p| p.join(&target))
                            .unwrap_or(target)
                    };
                    abs_target == expected_store_path
                })
                .unwrap_or(false)
        })
        .collect();

    if to_add.is_empty() {
        return Ok(());
    }

    // Without --yes, report rebuild candidates but do not absorb them.
    if !yes {
        for candidate in &to_add {
            actions.push(FixResult::NeedsConfirmation(format!(
                "manifest rebuild candidate '{candidate}': re-run with --yes to absorb"
            )));
        }
        return Ok(());
    }

    if dry_run {
        actions.push(FixResult::Fixed(format!(
            "[dry-run] would rebuild manifest with {} item(s): {}",
            to_add.len(),
            to_add.join(", ")
        )));
        return Ok(());
    }

    let now = context::now_iso8601();
    for orphan in &to_add {
        ctx.manifest.add(manifest::Item {
            item_id: Ulid::new().to_string(),
            origin_repo_id: ctx.repo_id.clone(),
            path: orphan.clone(),
            store_path: format!("items/{orphan}"),
            ownership_state: manifest::OwnershipState::Attached,
            created_at: now.clone(),
            updated_at: now.clone(),
        });
    }

    manifest::save(&ctx.repo_store, &ctx.manifest)?;
    actions.push(FixResult::Fixed(format!(
        "rebuilt manifest: added {} item(s): {}",
        to_add.len(),
        to_add.join(", ")
    )));
    Ok(())
}

/// Fixes an index root mismatch by updating the recorded root to the current
/// repository path.
#[cfg(test)]
fn fix_root_mismatch(ctx: &RepoContext, actions: &mut Vec<FixResult>, dry_run: bool) -> Result<()> {
    let idx = index::load(&ctx.config.store)?;
    let already_correct = idx
        .get(&ctx.repo_id)
        .map(|e| e.root.as_deref() == Some(ctx.repo_root.as_path()))
        .unwrap_or(false);

    if already_correct {
        actions.push(FixResult::Skipped("index root already correct".into()));
        return Ok(());
    }

    if dry_run {
        actions.push(FixResult::Fixed(format!(
            "[dry-run] would update index root to {}",
            ctx.repo_root.display()
        )));
        return Ok(());
    }

    // Reload with a mut binding so we can upsert.
    let mut idx = idx;
    let existing = idx.get(&ctx.repo_id).cloned();
    let git_dir = existing
        .as_ref()
        .and_then(|e| e.git_dir.clone())
        .unwrap_or_else(|| ctx.git_common_dir.clone());
    let git_common_dir = existing
        .as_ref()
        .and_then(|e| e.git_common_dir.clone())
        .unwrap_or_else(|| git_dir.clone());
    let store_dir = existing
        .as_ref()
        .map(|e| e.repo_store_dir.clone())
        .unwrap_or_else(|| ctx.repo_id.clone());

    idx.upsert(
        &ctx.repo_id,
        index::RepoEntry {
            root: Some(ctx.repo_root.clone()),
            git_dir: Some(git_dir),
            git_common_dir: Some(git_common_dir),
            repo_store_dir: store_dir,
            last_seen_at: context::now_iso8601(),
        },
    );
    index::save(&ctx.config.store, &idx)?;

    actions.push(FixResult::Fixed(format!(
        "updated index root to {}",
        ctx.repo_root.display()
    )));
    Ok(())
}

/// Ensures every manifested path appears in the ignore backend.
#[cfg(test)]
fn fix_exclude_entries(
    ctx: &RepoContext,
    ignore: &dyn IgnoreBackend,
    actions: &mut Vec<FixResult>,
    dry_run: bool,
) -> Result<()> {
    let mut missing: Vec<&str> = Vec::new();
    for item in &ctx.manifest.items {
        if !ignore.has_entry(&ctx.repo_root, &item.path)? {
            missing.push(&item.path);
        }
    }

    if missing.is_empty() {
        actions.push(FixResult::Skipped("all exclude entries present".into()));
        return Ok(());
    }

    if dry_run {
        actions.push(FixResult::Fixed(format!(
            "[dry-run] would add {} path(s) to .git/info/exclude: {}",
            missing.len(),
            missing.join(", ")
        )));
        return Ok(());
    }

    ignore.add_entries(&ctx.repo_root, &missing)?;
    actions.push(FixResult::Fixed(format!(
        "added {} path(s) to .git/info/exclude: {}",
        missing.len(),
        missing.join(", ")
    )));
    Ok(())
}

/// Iterates all manifest items and calls [`repair::repair`] for each.
/// Failures are recorded as [`FixResult::Failed`] rather than propagated.
#[cfg(test)]
fn fix_symlinks(
    ctx: &RepoContext,
    link: &dyn LinkStrategy,
    actions: &mut Vec<FixResult>,
    data_loss_warnings: &mut Vec<String>,
    dry_run: bool,
) {
    for item in &ctx.manifest.items {
        let abs_path = ctx.repo_root.join(&item.path);
        match repair::repair_report(ctx, &abs_path, link, dry_run, false).map(|r| r.outcome) {
            Ok(RepairOutcome::AlreadyHealthy) => {
                // Healthy items are not listed to keep output concise.
            }
            Ok(RepairOutcome::LinkRecreated) => {
                actions.push(FixResult::Fixed(format!(
                    "recreated symlink for '{}'",
                    item.path
                )));
            }
            Ok(RepairOutcome::StoreMissing) => {
                let msg = format!(
                    "'{}': store item missing — data may be lost. Restore manually and re-add.",
                    item.path
                );
                data_loss_warnings.push(msg.clone());
                actions.push(FixResult::CannotFix(msg));
            }
            Ok(RepairOutcome::NotManaged) => {
                // Should not happen (we are iterating the manifest), ignore.
            }
            Err(e) => {
                actions.push(FixResult::Failed(format!("'{}': {e}", item.path)));
            }
        }
    }
}

/// Reports unreferenced store items without deleting them.
///
/// Conservative GC only deletes manifest entries explicitly marked
/// `OwnershipState::Orphaned`, via `store gc`. A bare file under `items/` is
/// not enough proof that deletion is safe.
#[cfg(test)]
fn handle_orphans(ctx: &RepoContext, yes: bool, actions: &mut Vec<FixResult>, dry_run: bool) {
    for orphan in collect_orphan_store_items(ctx) {
        let prefix = if dry_run { "[dry-run] " } else { "" };
        let suffix = if yes {
            "; --yes does not delete unclassified store files"
        } else {
            "; conservative GC requires manifest ownership_state=orphaned"
        };
        actions.push(FixResult::Skipped(format!(
            "{prefix}unreferenced store item '{orphan}' left untouched{suffix}"
        )));
    }
}
