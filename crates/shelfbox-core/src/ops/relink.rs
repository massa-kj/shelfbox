use std::path::Path;

use crate::{
    context::{self, RepoContext},
    error::{AppError, Result},
    link::LinkStrategy,
    plan::item_relink::{ItemRelinkPlan, ItemRelinkReport},
    store::manifest::{self, OwnershipState},
};

use super::path::repo_relative_string;

pub use crate::plan::item_relink::RelinkOutcome;

pub(crate) fn relink_report(
    ctx: &mut RepoContext,
    abs_path: &Path,
    dry_run: bool,
    link: &dyn LinkStrategy,
) -> Result<ItemRelinkReport> {
    let plan = build_relink_plan(ctx, abs_path, link)?;

    if dry_run {
        return Ok(ItemRelinkReport {
            plan,
            outcome: RelinkOutcome::WouldRelink,
            dry_run,
        });
    }

    let outcome = execute_relink_plan(ctx, &plan, link)?;

    Ok(ItemRelinkReport {
        plan,
        outcome,
        dry_run,
    })
}

fn build_relink_plan(
    ctx: &RepoContext,
    abs_path: &Path,
    link: &dyn LinkStrategy,
) -> Result<ItemRelinkPlan> {
    // ── Resolve repo-relative path ────────────────────────────────────────
    let rel_str = repo_relative_string(&ctx.repo_root, abs_path)?;

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

    Ok(ItemRelinkPlan {
        path: rel_str,
        abs_path: abs_path.to_path_buf(),
        store_path,
        symlink_ok,
    })
}

fn execute_relink_plan(
    ctx: &mut RepoContext,
    plan: &ItemRelinkPlan,
    link: &dyn LinkStrategy,
) -> Result<RelinkOutcome> {
    // ── Execute ───────────────────────────────────────────────────────────
    let outcome = if plan.symlink_ok {
        // Symlink is already correct; only the manifest state needs updating.
        RelinkOutcome::StateUpdated
    } else {
        // Remove stale symlink if present, then recreate.
        if plan.abs_path.symlink_metadata().is_ok() {
            link.remove(&plan.abs_path)?;
        }
        link.create(&plan.store_path, &plan.abs_path)?;
        RelinkOutcome::Relinked
    };

    // ── Transition ownership_state: Detached -> Attached ──────────────────
    let now = context::now_iso8601();
    ctx.manifest
        .set_ownership_state(&plan.path, OwnershipState::Attached, &now);
    manifest::save(&ctx.repo_store, &ctx.manifest)?;

    Ok(outcome)
}
