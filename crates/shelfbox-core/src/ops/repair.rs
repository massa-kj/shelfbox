use std::path::Path;

use crate::{
    context::{self, RepoContext},
    error::{AppError, Result},
    ignore::GitInfoExclude,
    link::LinkStrategy,
    plan::item_repair::ItemRepairReport,
    plan::repo_repair::{RepoRepairPlan, RepoRepairSymlinkAction},
    policy::repair_policy::{self, SymlinkRepairDecision},
    store::{
        index::{self, RepoEntry},
        manifest,
    },
};

pub use crate::plan::item_repair::RepairOutcome;
pub use crate::plan::repo_repair::RepairRepoReport;

use super::path::repo_relative_string;

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
/// When `dry_run` is `true`, returns the validated report without making
/// changes.
pub fn repair(
    ctx: &RepoContext,
    abs_path: &Path,
    link: &dyn LinkStrategy,
    dry_run: bool,
    force: bool,
) -> Result<RepairOutcome> {
    repair_report(ctx, abs_path, link, dry_run, force).map(|report| report.outcome)
}

pub(crate) fn repair_report(
    ctx: &RepoContext,
    abs_path: &Path,
    link: &dyn LinkStrategy,
    dry_run: bool,
    force: bool,
) -> Result<ItemRepairReport> {
    let action = build_repair_action(ctx, abs_path, link, force)?;
    let outcome = repair_outcome(&action);

    if !dry_run {
        execute_repair_action(&action, link)?;
    }

    Ok(ItemRepairReport {
        action,
        outcome,
        dry_run,
    })
}

fn build_repair_action(
    ctx: &RepoContext,
    abs_path: &Path,
    link: &dyn LinkStrategy,
    force: bool,
) -> Result<RepoRepairSymlinkAction> {
    let path = repo_relative_string(&ctx.repo_root, abs_path)?;

    let item = match ctx.manifest.get(&path) {
        Some(item) => item,
        None => return Ok(RepoRepairSymlinkAction::NotManaged { path }),
    };
    let store_path = ctx.repo_store.join(&item.store_path);

    if !store_path.exists() {
        return Ok(RepoRepairSymlinkAction::StoreMissing { path });
    }

    let is_managed_link = link.is_managed_link(abs_path, &ctx.config.store);
    let is_link = link.is_link(abs_path);
    let actual_target = if !force && is_link {
        link.read_target(abs_path).ok().map(|actual_target| {
            if actual_target.is_absolute() {
                actual_target
            } else {
                abs_path
                    .parent()
                    .map(|parent| parent.join(&actual_target))
                    .unwrap_or(actual_target)
            }
        })
    } else {
        None
    };

    match repair_policy::decide_symlink_repair(
        is_managed_link,
        abs_path.exists(),
        is_link,
        actual_target,
        force,
    ) {
        SymlinkRepairDecision::AlreadyHealthy => {
            Ok(RepoRepairSymlinkAction::AlreadyHealthy { path })
        }
        SymlinkRepairDecision::Recreate => Ok(RepoRepairSymlinkAction::Recreate {
            path,
            abs_path: abs_path.to_path_buf(),
            store_path,
        }),
        SymlinkRepairDecision::RefuseRegularFile => Err(AppError::PathIsRegularFile {
            path: abs_path.to_path_buf(),
        }),
        SymlinkRepairDecision::RefuseUnexpectedTarget { actual_target } => {
            Err(AppError::RepairSymlinkTargetMismatch {
                path: abs_path.to_path_buf(),
                actual_target,
                expected_target: store_path,
            })
        }
    }
}

fn execute_repair_action(action: &RepoRepairSymlinkAction, link: &dyn LinkStrategy) -> Result<()> {
    let RepoRepairSymlinkAction::Recreate {
        abs_path,
        store_path,
        ..
    } = action
    else {
        return Ok(());
    };

    // Remove a stale/invalid symlink if one exists at the repo path. By this
    // point any non-symlink obstruction has already been rejected above.
    if link.is_link(abs_path) {
        link.remove(abs_path)?;
    }

    link.create(store_path, abs_path)
}

fn repair_outcome(action: &RepoRepairSymlinkAction) -> RepairOutcome {
    match action {
        RepoRepairSymlinkAction::Recreate { .. } => RepairOutcome::LinkRecreated,
        RepoRepairSymlinkAction::AlreadyHealthy { .. } => RepairOutcome::AlreadyHealthy,
        RepoRepairSymlinkAction::StoreMissing { .. } => RepairOutcome::StoreMissing,
        RepoRepairSymlinkAction::NotManaged { .. } | RepoRepairSymlinkAction::Failed { .. } => {
            RepairOutcome::NotManaged
        }
    }
}

fn build_repo_repair_plan(
    ctx: &RepoContext,
    link: &dyn LinkStrategy,
    force: bool,
    current: &context::CurrentGitContext,
    idx: &index::Index,
) -> Result<RepoRepairPlan> {
    let attached_paths = repair_policy::attached_item_paths(&ctx.manifest);

    let mut symlink_actions = Vec::new();
    for path in &attached_paths {
        let abs_path = ctx.repo_root.join(path);
        match build_repair_action(ctx, &abs_path, link, force) {
            Ok(action) => symlink_actions.push(action),
            Err(err) => symlink_actions.push(RepoRepairSymlinkAction::Failed {
                path: path.clone(),
                reason: err.to_string(),
            }),
        }
    }

    let ignore = GitInfoExclude;
    let exclude_updated = !ignore.entries_match(&ctx.repo_root, attached_paths.clone())?;
    let index_updated = idx
        .get(&ctx.repo_id)
        .map(|entry| {
            entry.root.as_deref() != Some(current.repo_root.as_path())
                || entry.git_dir.as_deref() != Some(current.git_dir.as_path())
                || entry.git_common_dir.as_deref() != Some(current.git_common_dir.as_path())
        })
        .unwrap_or(true);
    let hints_updated = if repair_policy::identity_hints_update_allowed(&symlink_actions) {
        identity_hints_need_update(ctx, current)
    } else {
        false
    };

    Ok(RepoRepairPlan {
        symlink_actions,
        exclude_paths: attached_paths,
        exclude_updated,
        index_updated,
        hints_updated,
    })
}

fn identity_hints_need_update(ctx: &RepoContext, current: &context::CurrentGitContext) -> bool {
    let mut planned = ctx.manifest.clone();
    if let Some(name) = current.repo_root.file_name().and_then(|name| name.to_str()) {
        planned.add_repo_name_hint(name);
    }
    if let Some(remote_hint) = &current.remote_hint {
        planned.add_remote_hint(remote_hint);
    }
    planned.touch_attached_at(context::now_iso8601());
    planned.identity_hints != ctx.manifest.identity_hints
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

    let plan = build_repo_repair_plan(ctx, link, force, &current, &idx)?;
    let mut report = RepairRepoReport::from_plan(plan);

    if dry_run {
        return Ok(report);
    }

    for action in &report.plan.symlink_actions {
        if !matches!(action, RepoRepairSymlinkAction::Recreate { .. }) {
            continue;
        }

        if let Err(err) = execute_repair_action(action, link) {
            report.symlinks_repaired = report.symlinks_repaired.saturating_sub(1);
            report
                .symlinks_failed
                .push((action.path().to_string(), err.to_string()));
        }
    }

    let ignore = GitInfoExclude;
    if report.plan.exclude_updated {
        ignore.update_entries(&ctx.repo_root, report.plan.exclude_paths.clone())?;
    }

    let mut idx = idx;
    if report.plan.index_updated {
        let entry = idx.get(&ctx.repo_id).cloned();
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

    if report.symlinks_failed.is_empty() && report.plan.hints_updated {
        let now = context::now_iso8601();
        if let Some(name) = current.repo_root.file_name().and_then(|name| name.to_str()) {
            ctx.manifest.add_repo_name_hint(name);
        }
        if let Some(remote_hint) = &current.remote_hint {
            ctx.manifest.add_remote_hint(remote_hint);
        }
        ctx.manifest.touch_attached_at(now);
        manifest::save(&ctx.repo_store, &ctx.manifest)?;
    } else if !report.symlinks_failed.is_empty() {
        report.hints_updated = false;
    }

    Ok(report)
}
