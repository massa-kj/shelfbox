use std::path::Path;

use crate::{
    context::{self, RepoContext},
    error::{AppError, Result},
    ignore::GitInfoExclude,
    link::LinkStrategy,
    store::{
        index::{self, RepoEntry},
        manifest::{self, OwnershipState},
    },
};

use super::path::repo_relative_string;

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

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RepairRepoReport {
    pub symlinks_repaired: usize,
    pub symlinks_already_healthy: usize,
    pub symlinks_failed: Vec<(String, String)>,
    pub exclude_updated: bool,
    pub index_updated: bool,
    pub hints_updated: bool,
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
    let rel_str = repo_relative_string(&ctx.repo_root, abs_path)?;

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
    // `abs_path.exists()` follows symlinks, so dangling symlinks (correctly
    // identified as links by `is_link()`) are not caught here.
    if !link.is_link(abs_path) && abs_path.exists() {
        return Err(AppError::PathIsRegularFile {
            path: abs_path.to_path_buf(),
        });
    }

    // ── Safety: refuse to silently overwrite a wrong-target symlink ───────
    // A symlink that exists but points outside the managed store is ambiguous:
    // it could be a stale link from a reclone, a different machine's store, or
    // a copied repo.  Overwriting it silently would mask those situations.
    // Require --force to proceed.
    if !force {
        if let Ok(actual_target) = link.read_target(abs_path) {
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
    // By this point any non-symlink obstruction has already been rejected
    // above, so `is_link()` is the correct predicate here.
    if link.is_link(abs_path) {
        link.remove(abs_path)?;
    }

    // Recreate the symlink pointing at the store item.
    link.create(&store_path, abs_path)?;

    Ok(RepairOutcome::LinkRecreated)
}

/// Repairs local working tree integration for the repository already
/// associated with `ctx.repo_id`.
///
/// This operation is ownership-neutral: it recreates symlinks for `Attached`
/// items, refreshes Git exclude/index metadata, and updates identity hints. It
/// does not reclaim, delete items, or change item ownership state.
pub fn repair_repo(
    ctx: &mut RepoContext,
    link: &dyn LinkStrategy,
    dry_run: bool,
    force: bool,
) -> Result<RepairRepoReport> {
    let current = context::current_git_context(&ctx.repo_root)?;
    let idx = index::load(&ctx.config.store)?;
    let associated_repo_id = context::resolve_existing_repo(&current, &idx)
        .ok_or_else(|| AppError::Internal("Run `shelfbox repo reclaim` first".to_string()))?;
    if associated_repo_id != ctx.repo_id {
        return Err(AppError::Internal(
            "Run `shelfbox repo reclaim` first".to_string(),
        ));
    }

    let mut report = RepairRepoReport::default();

    let attached_paths: Vec<String> = ctx
        .manifest
        .items
        .iter()
        .filter(|item| item.ownership_state == OwnershipState::Attached)
        .map(|item| item.path.clone())
        .collect();

    for path in &attached_paths {
        let abs_path = ctx.repo_root.join(path);
        match repair(ctx, &abs_path, link, dry_run, force) {
            Ok(RepairOutcome::LinkRecreated) => report.symlinks_repaired += 1,
            Ok(RepairOutcome::AlreadyHealthy) => report.symlinks_already_healthy += 1,
            Ok(RepairOutcome::StoreMissing) => {
                report
                    .symlinks_failed
                    .push((path.clone(), "store item missing".into()));
            }
            Ok(RepairOutcome::NotManaged) => {
                report
                    .symlinks_failed
                    .push((path.clone(), "not managed".into()));
            }
            Err(err) => report.symlinks_failed.push((path.clone(), err.to_string())),
        }
    }

    let ignore = GitInfoExclude;
    if !ignore.entries_match(&ctx.repo_root, attached_paths.clone())? {
        report.exclude_updated = true;
        if !dry_run {
            ignore.update_entries(&ctx.repo_root, attached_paths.clone())?;
        }
    }

    let mut idx = idx;
    let entry = idx.get(&ctx.repo_id).cloned();
    let index_needs_update = entry
        .as_ref()
        .map(|entry| {
            entry.root.as_deref() != Some(current.repo_root.as_path())
                || entry.git_dir.as_deref() != Some(current.git_dir.as_path())
                || entry.git_common_dir.as_deref() != Some(current.git_common_dir.as_path())
        })
        .unwrap_or(true);
    if index_needs_update {
        report.index_updated = true;
        if !dry_run {
            let repo_store_dir = entry.map(|entry| entry.repo_store_dir).unwrap_or_else(|| {
                ctx.repo_store
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| ctx.repo_id.clone())
            });
            idx.upsert(
                &ctx.repo_id,
                RepoEntry {
                    root: Some(current.repo_root.clone()),
                    git_dir: Some(current.git_dir.clone()),
                    git_common_dir: Some(current.git_common_dir.clone()),
                    repo_store_dir,
                    last_seen_at: context::now_iso8601(),
                },
            );
            index::save(&ctx.config.store, &idx)?;
        }
    }

    let before_hints = ctx.manifest.identity_hints.clone();
    let now = context::now_iso8601();
    if let Some(name) = current.repo_root.file_name().and_then(|name| name.to_str()) {
        ctx.manifest.add_repo_name_hint(name);
    }
    if let Some(remote_hint) = &current.remote_hint {
        ctx.manifest.add_remote_hint(remote_hint);
    }
    ctx.manifest.touch_attached_at(now);
    report.hints_updated = ctx.manifest.identity_hints != before_hints;
    if report.hints_updated && !dry_run {
        manifest::save(&ctx.repo_store, &ctx.manifest)?;
    } else if dry_run {
        ctx.manifest.identity_hints = before_hints;
    }

    Ok(report)
}
