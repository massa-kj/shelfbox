use std::path::Path;

use crate::{
    context::RepoContext,
    error::{AppError, Result},
    link::LinkStrategy,
};

/// The outcome of a single repair attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum RepairOutcome {
    /// The symlink was recreated or relinked to the correct target.
    LinkRecreated,
    /// The item was already healthy; no action was taken.
    AlreadyHealthy,
    /// The store-side file is missing; cannot repair without data recovery.
    StoreMissing,
    /// The path is not recorded in the manifest.
    NotManaged,
}

/// Attempts to repair the symlink for a single shelved item.
///
/// # Safety guards
///
/// - If a regular file (not a symlink) exists at `abs_path`, the function
///   returns [`AppError::PathIsRegularFile`] to prevent overwriting user data.
/// - If a symlink exists at `abs_path` but points to an unexpected target,
///   the function returns [`AppError::RepairSymlinkTargetMismatch`] unless
///   `force` is `true`.  This protects against silently masking a wrong
///   machine, stale store, or copied-repo situation.
/// - If the store-side copy is missing, returns [`RepairOutcome::StoreMissing`]
///   without touching the filesystem.
///
/// # Dry-run
///
/// When `dry_run` is `true`, prints what would happen without making changes.
pub fn repair(
    ctx: &RepoContext,
    abs_path: &Path,
    link: &dyn LinkStrategy,
    dry_run: bool,
    force: bool,
) -> Result<RepairOutcome> {
    // ── Resolve repo-relative path ────────────────────────────────────────
    let rel_path =
        abs_path
            .strip_prefix(&ctx.repo_root)
            .map_err(|_| AppError::PathOutsideRepo {
                path: abs_path.to_path_buf(),
            })?;
    let rel_str = rel_path.to_string_lossy().into_owned();

    // ── Must be in the manifest ───────────────────────────────────────────
    let item = match ctx.manifest.get(&rel_str) {
        Some(i) => i,
        None => return Ok(RepairOutcome::NotManaged),
    };
    let store_path = ctx.repo_store.join(&item.store_path);

    // ── Store item must exist ─────────────────────────────────────────────
    if !store_path.exists() {
        return Ok(RepairOutcome::StoreMissing);
    }

    // ── Already healthy? ──────────────────────────────────────────────────
    if link.is_managed_link(abs_path, &ctx.config.store) {
        return Ok(RepairOutcome::AlreadyHealthy);
    }

    // ── Safety: refuse to overwrite a regular file ────────────────────────
    // A non-symlink entry at the repo path means the user may have placed
    // their own file there.  Overwriting it silently would cause data loss.
    if let Ok(meta) = abs_path.symlink_metadata() {
        if !meta.file_type().is_symlink() {
            return Err(AppError::PathIsRegularFile {
                path: abs_path.to_path_buf(),
            });
        }
    }

    // ── Safety: refuse to silently overwrite a wrong-target symlink ───────
    // A symlink that exists but points outside the managed store is ambiguous:
    // it could be a stale link from a reclone, a different machine's store, or
    // a copied repo.  Overwriting it silently would mask those situations.
    // Require --force to proceed.
    if !force {
        if let Ok(actual_target) = std::fs::read_link(abs_path) {
            // Resolve relative symlinks against the link's parent directory.
            let abs_actual = if actual_target.is_absolute() {
                actual_target.clone()
            } else {
                abs_path
                    .parent()
                    .map(|p| p.join(&actual_target))
                    .unwrap_or_else(|| actual_target.clone())
            };
            return Err(AppError::RepairSymlinkTargetMismatch {
                path: abs_path.to_path_buf(),
                actual_target: abs_actual,
                expected_target: store_path.clone(),
            });
        }
    }

    // ── Dry-run ───────────────────────────────────────────────────────────
    if dry_run {
        println!("[dry-run] repair '{rel_str}'");
        println!(
            "  recreate symlink {} → {}",
            abs_path.display(),
            store_path.display()
        );
        return Ok(RepairOutcome::LinkRecreated);
    }

    // ── Execute ───────────────────────────────────────────────────────────
    // Remove a stale/invalid symlink if one exists at the repo path.
    if abs_path.symlink_metadata().is_ok() {
        link.remove(abs_path)?;
    }

    // Recreate the symlink pointing at the store item.
    link.create(&store_path, abs_path)?;

    Ok(RepairOutcome::LinkRecreated)
}
