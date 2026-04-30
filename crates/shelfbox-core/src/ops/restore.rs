use std::path::Path;

use crate::{
    context::RepoContext,
    error::{AppError, Result},
    ignore::IgnoreBackend,
    link::LinkStrategy,
    store::manifest,
};

/// Restores `abs_path` from the store: removes the symlink and moves the item
/// back to its original location in the repository.
///
/// # Dry-run
/// When `dry_run` is `true`, prints what would happen without making changes.
///
/// # keep_ignore
/// When `true`, the `.git/info/exclude` entry is preserved after restoration
/// (useful when the user plans to re-shelve the file shortly afterwards).
///
/// # Errors
///
/// Returns an error if the path is not a managed symlink or the store item is
/// missing.
pub fn restore(
    ctx: &mut RepoContext,
    abs_path: &Path,
    dry_run: bool,
    keep_ignore: bool,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<()> {
    // ── Validation ───────────────────────────────────────────────────────────
    // Must be within the repository root.
    let rel_path =
        abs_path
            .strip_prefix(&ctx.repo_root)
            .map_err(|_| AppError::PathOutsideRepo {
                path: abs_path.to_path_buf(),
            })?;
    let rel_str = rel_path.to_string_lossy().into_owned();

    // Must be a shelfbox managed symlink.
    if !link.is_managed_link(abs_path, &ctx.config.store) {
        return Err(AppError::NotManagedLink {
            path: abs_path.to_path_buf(),
        });
    }

    // Resolve the absolute store path from the manifest entry.
    let store_path = ctx
        .manifest
        .get(&rel_str)
        .map(|i| ctx.repo_store.join(&i.store_path))
        .ok_or_else(|| {
            AppError::Internal(format!(
                "symlink at '{}' points into store but is not recorded in the manifest",
                abs_path.display()
            ))
        })?;

    // Store item must exist (a missing item means the symlink is dangling).
    if !store_path.exists() {
        return Err(AppError::StoreMissing {
            path: abs_path.to_path_buf(),
            store_path: store_path.clone(),
        });
    }

    // ── Dry-run ──────────────────────────────────────────────────────────────
    if dry_run {
        println!("[dry-run] restore '{rel_str}'");
        println!("  remove symlink {}", abs_path.display());
        println!("  move   {} → {}", store_path.display(), abs_path.display());
        if !keep_ignore {
            println!("  remove from exclude: {rel_str}");
        }
        return Ok(());
    }

    // ── Execute ──────────────────────────────────────────────────────────────
    // Remove the symlink.
    link.remove(abs_path)?;

    // Move the item back to the repository; recreate the symlink on failure.
    if let Err(e) = std::fs::rename(&store_path, abs_path) {
        let _ = link.create(&store_path, abs_path);
        return Err(AppError::io(abs_path, e));
    }

    // Remove from the manifest and persist.
    ctx.manifest.remove(&rel_str);
    manifest::save(&ctx.repo_store, &ctx.manifest)?;

    // Remove from the ignore backend unless the caller asked to keep it.
    if !keep_ignore {
        ignore.remove_entries(&ctx.repo_root, &[&rel_str])?;
    }

    Ok(())
}
