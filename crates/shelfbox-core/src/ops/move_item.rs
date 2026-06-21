use std::path::Path;

use crate::{
    context::{self, RepoContext},
    error::{AppError, Result},
    ignore::IgnoreBackend,
    link::LinkStrategy,
    plan::item_move::{ItemMovePlan, ItemMoveReport, ItemMoveWarning},
    store::manifest,
};

// ── Store-level rename helper ─────────────────────────────────────────────────

/// Moves `src` to `dst` atomically when both paths reside on the same
/// filesystem, falling back to a copy-sync-remove sequence for cross-device
/// moves.  Parent directories of `dst` are created as needed.
fn rename_store_item(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            // Cross-device: copy data, fsync, then remove the source.
            std::fs::copy(src, dst).map_err(|e| AppError::io(dst, e))?;
            {
                let f = std::fs::File::open(dst).map_err(|e| AppError::io(dst, e))?;
                f.sync_all().map_err(|e| AppError::io(dst, e))?;
            }
            std::fs::remove_file(src).map_err(|e| AppError::io(src, e))?;
            Ok(())
        }
        Err(e) => Err(AppError::io(src, e)),
    }
}

// ── Public operation ──────────────────────────────────────────────────────────

/// Renames a shelved item's tracked path without restoring and re-shelving it.
///
/// The operation is performed in five phases:
///
/// 1. **Validate** — check all pre-conditions.
/// 2. **Store move** — rename the store-side file atomically.
/// 3. **Link update** — swap the old symlink for a new one at `new_abs`.
/// 4. **Manifest rewrite** — update `path`, `store_path`, and `updated_at`.
/// 5. **Exclude rewrite** — remove old path and add new path in
///    `.git/info/exclude`.
///
/// Rollback is attempted (best-effort) if phase 3 or 4 fails after the store
/// move has succeeded.  Failure of phase 5 (exclude) is demoted to a warning
/// because `shelfbox item repair` can restore a missing exclude entry.
///
/// # Limitations
///
/// Moving directory items is not supported in this version.
///
/// # Errors
///
/// See [`AppError`] for the full list of failure variants.
pub fn move_item(
    ctx: &mut RepoContext,
    old_abs: &Path,
    new_abs: &Path,
    dry_run: bool,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<ItemMoveReport> {
    let plan = ItemMovePlan::build(ctx, old_abs, new_abs)?;

    if dry_run {
        return Ok(ItemMoveReport {
            plan,
            dry_run: true,
            warnings: Vec::new(),
        });
    }

    // ── Phase 2: store move ───────────────────────────────────────────────
    rename_store_item(&plan.old_store_path, &plan.new_store_path)?;

    // ── Phase 3: link update ──────────────────────────────────────────────
    let link_result = (|| -> Result<()> {
        link.remove(&plan.old_abs_path)?;
        link.create(&plan.new_store_path, &plan.new_abs_path)?;
        Ok(())
    })();

    if let Err(e) = link_result {
        // Roll back the store move (best-effort).
        let _ = rename_store_item(&plan.new_store_path, &plan.old_store_path);
        return Err(e);
    }

    // ── Phase 4: manifest rewrite ─────────────────────────────────────────
    let now = context::now_iso8601();
    ctx.manifest.rename(
        &plan.old_path,
        &plan.new_path,
        &plan.new_store_path_relative,
        &now,
    );

    if let Err(e) = manifest::save(&ctx.repo_store, &ctx.manifest) {
        // Roll back link update and store move (best-effort).
        let _ = link.remove(&plan.new_abs_path);
        let _ = link.create(&plan.old_store_path, &plan.old_abs_path);
        let _ = rename_store_item(&plan.new_store_path, &plan.old_store_path);
        return Err(e);
    }

    // ── Phase 5: exclude rewrite ──────────────────────────────────────────
    // Failure is non-fatal: the file is still correctly shelved.
    // The user can run `shelfbox item repair` to restore the exclude entry.
    let mut warnings = Vec::new();
    if let Err(e) = ignore.remove_entries(&ctx.repo_root, &[&plan.old_path]) {
        warnings.push(ItemMoveWarning::ExcludeRemoveFailed {
            path: plan.old_path.clone(),
            message: e.to_string(),
        });
    }
    if let Err(e) = ignore.add_entries(&ctx.repo_root, &[&plan.new_path]) {
        warnings.push(ItemMoveWarning::ExcludeAddFailed {
            path: plan.new_path.clone(),
            message: e.to_string(),
        });
    }

    Ok(ItemMoveReport {
        plan,
        dry_run: false,
        warnings,
    })
}
