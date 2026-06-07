use std::path::Path;

use crate::{
    context::{self, RepoContext},
    error::{AppError, Result},
    ignore::IgnoreBackend,
    link::LinkStrategy,
    store::manifest::{self, OwnershipState},
};

use super::path::repo_relative_string;

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
/// # keep_store
/// When `true`, only the manifest entry is removed.  The symlink and the
/// store-side item are left in place, making the store item an orphan that
/// will be collected by `repo gc`.
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
    keep_store: bool,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<()> {
    // ── Validation ───────────────────────────────────────────────────────────
    // Must be within the repository root.
    let rel_str = repo_relative_string(&ctx.repo_root, abs_path)?;

    // ── keep_store fast path ─────────────────────────────────────────────────
    // Transition the item to Detached: preserve the manifest entry for
    // ownership tracking while leaving the symlink and store item intact.
    // The item will NOT be auto-collected by `repo gc`; the user must
    // explicitly confirm GC for detached items.
    if keep_store {
        if !ctx.manifest.contains(&rel_str) {
            return Err(AppError::NotManagedLink {
                path: abs_path.to_path_buf(),
            });
        }

        if dry_run {
            println!("[dry-run] restore --keep-store '{rel_str}'");
            println!("  ownership_state: attached -> detached");
            println!("  (symlink and store item left in place)");
            if !keep_ignore {
                println!("  remove from exclude: {rel_str}");
            }
            return Ok(());
        }

        let now = context::now_iso8601();
        ctx.manifest
            .set_ownership_state(&rel_str, OwnershipState::Detached, &now);
        manifest::save(&ctx.repo_store, &ctx.manifest)?;

        if !keep_ignore {
            ignore.remove_entries(&ctx.repo_root, &[&rel_str])?;
        }
        return Ok(());
    }

    // ── Normal restore ───────────────────────────────────────────────────────
    // Using symlink_metadata to distinguish the three cases without following
    // the link, so we can give a precise error in each situation.
    match std::fs::symlink_metadata(abs_path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            // It is a symlink: verify it points into the shelfbox store.
            if !link.is_managed_link(abs_path, &ctx.config.store) {
                return Err(AppError::NotManagedLink {
                    path: abs_path.to_path_buf(),
                });
            }
        }
        Ok(_) => {
            // A regular file or directory exists at the path: refuse to
            // overwrite it to prevent data loss.
            return Err(AppError::RestoreDestinationExists {
                path: abs_path.to_path_buf(),
            });
        }
        Err(_) => {
            // Nothing at this path.
            return Err(AppError::NotManagedLink {
                path: abs_path.to_path_buf(),
            });
        }
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

// ── Directory namespace restore ───────────────────────────────────────────────

/// Outcome for a single item during [`restore_namespace`].
#[derive(Debug)]
pub enum NsRestoreItemOutcome {
    /// Item was successfully restored.
    Restored,
    /// Item would be restored (dry-run mode).
    WouldRestore,
    /// Restore failed.
    Failed(String),
}

/// Summary of a [`restore_namespace`] operation.
#[derive(Debug)]
pub struct NamespaceRestoreResult {
    /// The namespace path that was operated on.
    pub ns_path: String,
    /// Per-item outcomes.
    pub results: Vec<(String, NsRestoreItemOutcome)>,
    /// Whether the namespace entry was removed from the manifest after restoring.
    pub namespace_removed: bool,
}

/// Restores all items that belong to the directory namespace at `ns_path`.
///
/// `ns_path` must be a repo-relative directory path ending with `/`, matching
/// the form used when the namespace was registered (e.g. `"secrets/"`).
///
/// After all members are successfully restored the namespace entry is removed
/// from the manifest automatically.
///
/// # Dry-run
/// When `dry_run` is `true`, prints what would happen without making changes.
pub fn restore_namespace(
    ctx: &mut RepoContext,
    ns_path: &str,
    dry_run: bool,
    keep_ignore: bool,
    keep_store: bool,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<NamespaceRestoreResult> {
    // Verify the namespace is registered.
    if !ctx.manifest.namespaces.iter().any(|n| n.path == ns_path) {
        return Err(AppError::NamespaceNotFound {
            path: ns_path.to_owned(),
        });
    }

    // Collect member paths (clone to release the borrow on ctx.manifest).
    let member_paths: Vec<String> = ctx
        .manifest
        .namespace_members(ns_path)
        .map(|i| i.path.clone())
        .collect();

    let mut results: Vec<(String, NsRestoreItemOutcome)> = Vec::new();

    if dry_run {
        for member_path in &member_paths {
            results.push((member_path.clone(), NsRestoreItemOutcome::WouldRestore));
        }
        return Ok(NamespaceRestoreResult {
            ns_path: ns_path.to_owned(),
            results,
            namespace_removed: false,
        });
    }

    // Restore each member individually.
    for member_path in &member_paths {
        let abs_path = ctx.repo_root.join(member_path);
        match restore(ctx, &abs_path, false, keep_ignore, keep_store, link, ignore) {
            Ok(()) => results.push((member_path.clone(), NsRestoreItemOutcome::Restored)),
            Err(e) => results.push((
                member_path.clone(),
                NsRestoreItemOutcome::Failed(e.to_string()),
            )),
        }
    }

    // Remove namespace entry if no members remain.
    let namespace_removed = if ctx.manifest.remove_namespace_if_empty(ns_path) {
        manifest::save(&ctx.repo_store, &ctx.manifest)?;
        true
    } else {
        false
    };

    Ok(NamespaceRestoreResult {
        ns_path: ns_path.to_owned(),
        results,
        namespace_removed,
    })
}
