use std::path::Path;

use crate::{
    context::{self, RepoContext},
    error::{AppError, Result},
    ignore::IgnoreBackend,
    link::LinkStrategy,
    plan::item_restore::{ItemRestoreAction, ItemRestorePlan, ItemRestoreReport},
    store::manifest::{self, OwnershipState},
};

/// Restores `abs_path` from the store: removes the symlink and moves the item
/// back to its original location in the repository.
///
/// # Dry-run
/// When `dry_run` is `true`, returns the validated plan without making changes.
///
/// # keep_ignore
/// When `true`, the `.git/info/exclude` entry is preserved after restoration
/// (useful when the user plans to re-shelve the file shortly afterwards).
///
/// # keep_store
/// When `true`, the manifest entry is retained and marked `Detached`. The
/// symlink and store-side item are left in place.
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
) -> Result<ItemRestoreReport> {
    let plan = ItemRestorePlan::build(ctx, abs_path, keep_ignore, keep_store, link)?;

    if dry_run {
        return Ok(ItemRestoreReport {
            plan,
            dry_run: true,
        });
    }

    match &plan.action {
        ItemRestoreAction::DetachKeepStore => {
            // Transition the item to Detached: preserve the manifest entry for
            // ownership tracking while leaving the symlink and store item
            // intact. Detached items are protected from conservative GC.
            let now = context::now_iso8601();
            ctx.manifest
                .set_ownership_state(&plan.path, OwnershipState::Detached, &now);
            manifest::save(&ctx.repo_store, &ctx.manifest)?;

            if !plan.keep_ignore {
                ignore.remove_entries(&ctx.repo_root, &[&plan.path])?;
            }
        }
        ItemRestoreAction::RestoreFile => {
            // Remove the symlink.
            link.remove(&plan.abs_path)?;

            // Move the item back to the repository; recreate the symlink on
            // failure.
            if let Err(e) = std::fs::rename(&plan.store_path, &plan.abs_path) {
                let _ = link.create(&plan.store_path, &plan.abs_path);
                return Err(AppError::io(&plan.abs_path, e));
            }

            // Remove from the manifest and persist.
            ctx.manifest.remove(&plan.path);
            manifest::save(&ctx.repo_store, &ctx.manifest)?;

            // Remove from the ignore backend unless the caller asked to keep it.
            if !plan.keep_ignore {
                ignore.remove_entries(&ctx.repo_root, &[&plan.path])?;
            }
        }
    }

    Ok(ItemRestoreReport {
        plan,
        dry_run: false,
    })
}

// ── Directory namespace restore ───────────────────────────────────────────────

/// Outcome for a single item during namespace restore.
#[derive(Debug)]
pub enum NsRestoreItemOutcome {
    /// Item was successfully restored.
    Restored,
    /// Item would be restored (dry-run mode).
    WouldRestore,
    /// Restore failed.
    Failed(String),
}

/// Summary of a namespace restore operation.
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
    // v0.7.0 no longer persists namespace entries. Directory restore derives
    // membership from item paths.
    let member_paths: Vec<String> = ctx
        .manifest
        .items
        .iter()
        .filter(|i| i.path.starts_with(ns_path))
        .map(|i| i.path.clone())
        .collect();

    if member_paths.is_empty() {
        return Err(AppError::NamespaceNotFound {
            path: ns_path.to_owned(),
        });
    }

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
            Ok(_) => results.push((member_path.clone(), NsRestoreItemOutcome::Restored)),
            Err(e) => results.push((
                member_path.clone(),
                NsRestoreItemOutcome::Failed(e.to_string()),
            )),
        }
    }

    Ok(NamespaceRestoreResult {
        ns_path: ns_path.to_owned(),
        results,
        namespace_removed: false,
    })
}
