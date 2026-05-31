use std::path::Path;

use crate::{
    context::{self, RepoContext},
    error::{AppError, Result},
    link::LinkStrategy,
    store::manifest::{self, OwnershipState},
};

/// Outcome of a [`relink`] operation.
#[derive(Debug, PartialEq, Eq)]
pub enum RelinkOutcome {
    /// Symlink was recreated and ownership_state transitioned to `Attached`.
    Relinked,
    /// Symlink already pointed to the correct store path; only state was updated.
    StateUpdated,
    /// Dry-run: item would be relinked.
    WouldRelink,
}

/// Transitions a `Detached` item back to `Attached` by ensuring its symlink
/// exists and updating `ownership_state` in the manifest.
///
/// # Distinction from `repair`
///
/// `repair` is ownership-neutral: it restores a broken symlink for an
/// `Attached` item without mutating `ownership_state`.  `relink` is
/// exclusively for `Detached` items and explicitly transitions ownership
/// state from `Detached` to `Attached` (spec §6.1).
///
/// # Dry-run
///
/// When `dry_run` is `true`, prints what would happen without making changes.
///
/// # Errors
///
/// - [`AppError::PathOutsideRepo`] — path is not within the repository root.
/// - [`AppError::NotManagedLink`] — path is not recorded in the manifest.
/// - [`AppError::RelinkNotDetached`] — item exists but is not in `Detached` state.
/// - [`AppError::StoreMissing`] — store-side file is missing; cannot relink.
/// - [`AppError::PathIsRegularFile`] — a regular file exists at the repo path.
pub fn relink(
    ctx: &mut RepoContext,
    abs_path: &Path,
    dry_run: bool,
    link: &dyn LinkStrategy,
) -> Result<RelinkOutcome> {
    // ── Resolve repo-relative path ────────────────────────────────────────
    let rel_path =
        abs_path
            .strip_prefix(&ctx.repo_root)
            .map_err(|_| AppError::PathOutsideRepo {
                path: abs_path.to_path_buf(),
            })?;
    let rel_str = rel_path.to_string_lossy().into_owned();

    // ── Must be in the manifest ───────────────────────────────────────────
    let item = ctx
        .manifest
        .get(&rel_str)
        .ok_or_else(|| AppError::NotManagedLink {
            path: abs_path.to_path_buf(),
        })?;

    // ── Must be Detached ──────────────────────────────────────────────────
    if item.ownership_state != OwnershipState::Detached {
        return Err(AppError::RelinkNotDetached {
            path: abs_path.to_path_buf(),
            actual_state: format!("{:?}", item.ownership_state),
        });
    }

    let store_path = ctx.repo_store.join(&item.store_path);

    // ── Store item must exist ─────────────────────────────────────────────
    if !store_path.exists() {
        return Err(AppError::StoreMissing {
            path: abs_path.to_path_buf(),
            store_path: store_path.clone(),
        });
    }

    // ── Safety: refuse to overwrite a regular file ────────────────────────
    if let Ok(meta) = abs_path.symlink_metadata() {
        if !meta.file_type().is_symlink() {
            return Err(AppError::PathIsRegularFile {
                path: abs_path.to_path_buf(),
            });
        }
    }

    // ── Detect whether symlink is already correct ─────────────────────────
    let symlink_ok = link.is_managed_link(abs_path, &ctx.config.store);

    if dry_run {
        if symlink_ok {
            println!("[dry-run] relink '{rel_str}'");
            println!("  symlink already correct — update ownership_state: detached -> attached");
        } else {
            println!("[dry-run] relink '{rel_str}'");
            println!(
                "  recreate symlink {} -> {}",
                abs_path.display(),
                store_path.display()
            );
            println!("  ownership_state: detached -> attached");
        }
        return Ok(RelinkOutcome::WouldRelink);
    }

    // ── Execute ───────────────────────────────────────────────────────────
    let outcome = if symlink_ok {
        // Symlink is already correct; only the manifest state needs updating.
        RelinkOutcome::StateUpdated
    } else {
        // Remove stale symlink if present, then recreate.
        if abs_path.symlink_metadata().is_ok() {
            link.remove(abs_path)?;
        }
        link.create(&store_path, abs_path)?;
        RelinkOutcome::Relinked
    };

    // ── Transition ownership_state: Detached -> Attached ──────────────────
    let now = context::now_iso8601();
    ctx.manifest
        .set_ownership_state(&rel_str, OwnershipState::Attached, &now);
    manifest::save(&ctx.repo_store, &ctx.manifest)?;

    Ok(outcome)
}
