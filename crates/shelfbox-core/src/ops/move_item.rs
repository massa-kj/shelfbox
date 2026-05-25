use std::path::Path;

use crate::{
    context::{self, RepoContext},
    error::{AppError, Result},
    git,
    ignore::IgnoreBackend,
    link::LinkStrategy,
    store::manifest::{self, ItemKind},
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
) -> Result<()> {
    // ── Phase 1: validate ─────────────────────────────────────────────────

    // Both paths must reside inside the repository root.
    let old_rel = old_abs
        .strip_prefix(&ctx.repo_root)
        .map_err(|_| AppError::PathOutsideRepo {
            path: old_abs.to_path_buf(),
        })?;
    let new_rel = new_abs
        .strip_prefix(&ctx.repo_root)
        .map_err(|_| AppError::PathOutsideRepo {
            path: new_abs.to_path_buf(),
        })?;

    let old_rel_str = old_rel.to_string_lossy().into_owned();
    let new_rel_str = new_rel.to_string_lossy().into_owned();

    // Source must be managed.
    let item = ctx
        .manifest
        .get(&old_rel_str)
        .ok_or_else(|| AppError::NotManagedLink {
            path: old_abs.to_path_buf(),
        })?
        .clone();

    // Directory moves are not yet supported.
    if item.kind == ItemKind::Directory {
        return Err(AppError::MoveDirectoryUnsupported);
    }

    // Destination must not already be managed.
    if ctx.manifest.contains(&new_rel_str) {
        return Err(AppError::AlreadyManaged {
            path: new_abs.to_path_buf(),
        });
    }

    // Destination must not exist on the filesystem (prevents data loss).
    if new_abs.exists() {
        return Err(AppError::MoveDestinationExists {
            path: new_abs.to_path_buf(),
        });
    }

    // Destination must not be tracked by Git.
    if git::is_tracked(&ctx.repo_root, new_abs)? {
        return Err(AppError::PathIsTracked {
            path: new_abs.to_path_buf(),
        });
    }

    // Compute expected store paths.
    let old_store_path = ctx.repo_store.join(&item.store_path);
    let new_store_path_rel = format!("items/{new_rel_str}");
    let new_store_path = ctx.repo_store.join(&new_store_path_rel);

    // Source store item must exist.
    if !old_store_path.exists() {
        return Err(AppError::StoreMissing {
            path: old_abs.to_path_buf(),
            store_path: old_store_path.clone(),
        });
    }

    // Source symlink must point to the expected store path.
    // If it doesn't, the state is inconsistent and `item repair` should be
    // run first.
    match std::fs::read_link(old_abs) {
        Ok(target) if target == old_store_path => {}
        _ => {
            return Err(AppError::MoveSourceSymlinkMismatch {
                path: old_abs.to_path_buf(),
            });
        }
    }

    // ── Dry-run output ────────────────────────────────────────────────────
    if dry_run {
        println!("[dry-run] move '{old_rel_str}' → '{new_rel_str}'");
        println!(
            "  store   {} → {}",
            old_store_path.display(),
            new_store_path.display()
        );
        println!("  symlink {} → {}", old_abs.display(), new_abs.display());
        println!("  manifest: update path and store_path");
        println!("  exclude:  remove '{old_rel_str}', add '{new_rel_str}'");
        return Ok(());
    }

    // ── Phase 2: store move ───────────────────────────────────────────────
    rename_store_item(&old_store_path, &new_store_path)?;

    // ── Phase 3: link update ──────────────────────────────────────────────
    let link_result = (|| -> Result<()> {
        link.remove(old_abs)?;
        link.create(&new_store_path, new_abs)?;
        Ok(())
    })();

    if let Err(e) = link_result {
        // Roll back the store move (best-effort).
        let _ = rename_store_item(&new_store_path, &old_store_path);
        return Err(e);
    }

    // ── Phase 4: manifest rewrite ─────────────────────────────────────────
    let now = context::now_iso8601();
    ctx.manifest
        .rename(&old_rel_str, &new_rel_str, &new_store_path_rel, &now);

    if let Err(e) = manifest::save(&ctx.repo_store, &ctx.manifest) {
        // Roll back link update and store move (best-effort).
        let _ = link.remove(new_abs);
        let _ = link.create(&old_store_path, old_abs);
        let _ = rename_store_item(&new_store_path, &old_store_path);
        return Err(e);
    }

    // ── Phase 5: exclude rewrite ──────────────────────────────────────────
    // Failure is non-fatal: the file is still correctly shelved.
    // The user can run `shelfbox item repair` to restore the exclude entry.
    if let Err(e) = ignore.remove_entries(&ctx.repo_root, &[&old_rel_str]) {
        eprintln!(
            "warning: failed to remove '{old_rel_str}' from .git/info/exclude: {e}\n\
             hint: run 'shelfbox item repair' to restore the exclude entry"
        );
    }
    if let Err(e) = ignore.add_entries(&ctx.repo_root, &[&new_rel_str]) {
        eprintln!(
            "warning: failed to add '{new_rel_str}' to .git/info/exclude: {e}\n\
             hint: run 'shelfbox item repair' to restore the exclude entry"
        );
    }

    Ok(())
}
