use std::{collections::BTreeSet, path::Path};

use crate::{
    context::{self, RepoContext},
    domain::{
        manifest::Item,
        materialization::{CopyContentState, MaterializationStrategy},
        ownership::OwnershipState,
        path::{RepoRelativePath, StoreRelativePath},
    },
    error::{AppError, Result},
    failpoint::{self, Failpoint},
    fs::{
        materializer::{
            DefaultMaterializer, InspectionPurpose, MaterializationAction,
            MaterializationInspectionRequest, MaterializationLocation, Materializer,
            MutationJournal, RepoEntryKind,
        },
        mutation_journal::RepairMutationJournal,
    },
    git,
    ignore::{GitInfoExclude, IgnoreBackend},
    plan::item_repair::ItemRepairReport,
    plan::repo_repair::{RepoRepairPlan, RepoRepairSymlinkAction},
    policy::repair_policy::{
        self, MaterializationRepairDecision, RepairMaterializationState, SymlinkRepairDecision,
    },
    store::{
        index::{self, RepoEntry},
        manifest,
    },
};

pub use crate::plan::item_repair::RepairOutcome;
pub use crate::plan::repo_repair::RepairRepoReport;

use super::path::repo_relative_string;

/// Item repair never changes the target managed exclude entry. It requires the
/// entry to already exist, then creates only a missing materialization through
/// the configured strategy.
pub(crate) fn repair_report<L: ?Sized>(
    ctx: &RepoContext,
    abs_path: &Path,
    _legacy_link: &L,
    dry_run: bool,
    force: bool,
) -> Result<ItemRepairReport> {
    let ignore = GitInfoExclude;
    repair_report_with_ignore(ctx, abs_path, &ignore, dry_run, force)
}

pub(crate) fn repair_report_with_ignore(
    ctx: &RepoContext,
    abs_path: &Path,
    ignore: &dyn IgnoreBackend,
    dry_run: bool,
    force: bool,
) -> Result<ItemRepairReport> {
    let path = repo_relative_string(&ctx.repo_root, abs_path)?;
    let target_excluded = ctx
        .manifest
        .get(&path)
        .map(|item| {
            if item.ownership_state == OwnershipState::Attached {
                ignore.has_entry(&ctx.repo_root, &path)
            } else {
                Ok(true)
            }
        })
        .transpose()?
        .unwrap_or(false);
    let action = build_repair_action(ctx, abs_path, force, target_excluded)?;
    let outcome = repair_outcome(&action);

    if !dry_run {
        execute_repair_action(ctx, &action, ignore, force)?;
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
    force: bool,
    target_excluded: bool,
) -> Result<RepoRepairSymlinkAction> {
    let path = repo_relative_string(&ctx.repo_root, abs_path)?;
    let item = match ctx.manifest.get(&path) {
        Some(item) => item,
        None => return Ok(RepoRepairSymlinkAction::NotManaged { path }),
    };
    build_repair_action_for_item(ctx, item, abs_path, force, target_excluded)
}

fn build_repair_action_for_item(
    ctx: &RepoContext,
    item: &Item,
    abs_path: &Path,
    force: bool,
    target_excluded: bool,
) -> Result<RepoRepairSymlinkAction> {
    let path = item.path.clone();
    if item.ownership_state != OwnershipState::Attached {
        return Ok(RepoRepairSymlinkAction::DetachedDisabled { path });
    }
    if !target_excluded {
        return Err(AppError::Internal(
            "managed repair exclude is missing; repair is exclude-neutral".into(),
        ));
    }

    let location = materialization_location(ctx, item)?;
    let materializer = DefaultMaterializer::new(ctx.repo_root.clone(), ctx.config.store.clone());
    let facts = materializer.inspect(MaterializationInspectionRequest {
        location,
        purpose: InspectionPurpose::Planning,
    })?;
    if !facts.store_exists {
        return Ok(RepoRepairSymlinkAction::StoreMissing { path });
    }
    if !facts.store_regular || !facts.store_hardlink_free {
        return Err(AppError::UnsafeFilesystemEntry {
            path: ctx.repo_store.join(&item.store_path),
            reason: "repair store entry is not an isolated regular file",
        });
    }
    if facts.repo_entry_kind == RepoEntryKind::RegularFile && !facts.hardlink_free {
        return Err(AppError::HardlinkedFile {
            path: abs_path.to_path_buf(),
        });
    }

    let state = repair_materialization_state(&facts);
    match repair_policy::decide_materialization_repair(state) {
        MaterializationRepairDecision::AlreadyHealthy => {
            Ok(RepoRepairSymlinkAction::AlreadyHealthy { path })
        }
        MaterializationRepairDecision::Create => create_action(ctx, path, abs_path, item),
        MaterializationRepairDecision::CopyDiverged => {
            Ok(RepoRepairSymlinkAction::CopyDiverged { path })
        }
        MaterializationRepairDecision::StoreMissing => {
            Ok(RepoRepairSymlinkAction::StoreMissing { path })
        }
        MaterializationRepairDecision::RefuseRegular => Err(AppError::PathIsRegularFile {
            path: abs_path.to_path_buf(),
        }),
        MaterializationRepairDecision::DelegateSymlinkPolicy => {
            let actual_target = if force {
                None
            } else {
                facts.unmanaged_symlink_target.clone()
            };
            match repair_policy::decide_symlink_repair(false, true, true, actual_target, force) {
                SymlinkRepairDecision::Recreate => create_action(ctx, path, abs_path, item),
                SymlinkRepairDecision::RefuseRegularFile => Err(AppError::PathIsRegularFile {
                    path: abs_path.to_path_buf(),
                }),
                SymlinkRepairDecision::RefuseUnexpectedTarget { actual_target } => {
                    Err(AppError::RepairSymlinkTargetMismatch {
                        path: abs_path.to_path_buf(),
                        actual_target,
                        expected_target: ctx.repo_store.join(&item.store_path),
                    })
                }
                SymlinkRepairDecision::AlreadyHealthy => {
                    Ok(RepoRepairSymlinkAction::AlreadyHealthy { path })
                }
            }
        }
    }
}

fn create_action(
    ctx: &RepoContext,
    path: String,
    abs_path: &Path,
    item: &Item,
) -> Result<RepoRepairSymlinkAction> {
    let store_path = ctx.repo_store.join(&item.store_path);
    Ok(match ctx.config.materialization {
        MaterializationStrategy::Symlink => RepoRepairSymlinkAction::Recreate {
            path,
            abs_path: abs_path.to_path_buf(),
            store_path,
        },
        MaterializationStrategy::Copy => RepoRepairSymlinkAction::CreateCopy {
            path,
            abs_path: abs_path.to_path_buf(),
            store_path,
        },
    })
}

fn repair_materialization_state(
    facts: &crate::fs::materializer::MaterializationFacts,
) -> RepairMaterializationState {
    if !facts.store_exists {
        return RepairMaterializationState::StoreMissing;
    }
    match facts.repo_entry_kind {
        RepoEntryKind::Missing => RepairMaterializationState::Missing,
        RepoEntryKind::ManagedSymlink => RepairMaterializationState::ManagedSymlink,
        RepoEntryKind::UnmanagedSymlinkOrReparsePoint => {
            RepairMaterializationState::UnmanagedSymlink
        }
        RepoEntryKind::RegularFile => match facts.copy_content {
            CopyContentState::Equal => RepairMaterializationState::RegularEqual,
            CopyContentState::Diverged => RepairMaterializationState::RegularDiverged,
            CopyContentState::NotCompared
            | CopyContentState::Unreadable
            | CopyContentState::ComparisonFailed => RepairMaterializationState::RegularUnknown,
        },
        RepoEntryKind::Directory | RepoEntryKind::Other => RepairMaterializationState::Unsafe,
    }
}

fn materialization_location(ctx: &RepoContext, item: &Item) -> Result<MaterializationLocation> {
    let repo_path = RepoRelativePath::new(item.path.clone()).ok_or_else(|| {
        AppError::UnsafeFilesystemEntry {
            path: item.path.clone().into(),
            reason: "repair item repository path is not normalized",
        }
    })?;
    let store_absolute = ctx.repo_store.join(&item.store_path);
    let store_relative = store_absolute
        .strip_prefix(&ctx.config.store)
        .map_err(|_| AppError::UnsafeFilesystemEntry {
            path: store_absolute.clone(),
            reason: "repair item store path escapes the configured store root",
        })?;
    let store_path = StoreRelativePath::new(store_relative.to_string_lossy().replace('\\', "/"))
        .ok_or(AppError::UnsafeFilesystemEntry {
            path: store_absolute,
            reason: "repair item store path is not normalized",
        })?;
    Ok(MaterializationLocation::new(repo_path, store_path))
}

fn execute_repair_action(
    ctx: &RepoContext,
    action: &RepoRepairSymlinkAction,
    ignore: &dyn IgnoreBackend,
    force: bool,
) -> Result<()> {
    let (path, abs_path, store_path, strategy) = match action {
        RepoRepairSymlinkAction::Recreate {
            path,
            abs_path,
            store_path,
        } => (path, abs_path, store_path, MaterializationStrategy::Symlink),
        RepoRepairSymlinkAction::CreateCopy {
            path,
            abs_path,
            store_path,
        } => (path, abs_path, store_path, MaterializationStrategy::Copy),
        RepoRepairSymlinkAction::AlreadyHealthy { .. }
        | RepoRepairSymlinkAction::StoreMissing { .. }
        | RepoRepairSymlinkAction::CopyDiverged { .. }
        | RepoRepairSymlinkAction::DetachedDisabled { .. }
        | RepoRepairSymlinkAction::NotManaged { .. }
        | RepoRepairSymlinkAction::Failed { .. } => return Ok(()),
    };

    if !ignore.has_entry(&ctx.repo_root, path)? {
        return Err(AppError::Internal(
            "managed repair exclude is missing; repair is exclude-neutral".into(),
        ));
    }
    if git::is_tracked(&ctx.repo_root, abs_path)? {
        return Err(AppError::PathIsTracked {
            path: abs_path.clone(),
        });
    }

    let repo_path =
        RepoRelativePath::new(path.clone()).ok_or_else(|| AppError::UnsafeFilesystemEntry {
            path: abs_path.clone(),
            reason: "repair path is not normalized",
        })?;
    let relative_store = store_path
        .strip_prefix(&ctx.config.store)
        .ok()
        .and_then(|path| StoreRelativePath::new(path.to_string_lossy().replace('\\', "/")))
        .ok_or_else(|| AppError::UnsafeFilesystemEntry {
            path: store_path.clone(),
            reason: "repair store path escapes the configured store root",
        })?;
    let location = MaterializationLocation::new(repo_path, relative_store);
    let mut materializer =
        DefaultMaterializer::new(ctx.repo_root.clone(), ctx.config.store.clone());
    let fresh = materializer.inspect(MaterializationInspectionRequest {
        location: location.clone(),
        purpose: InspectionPurpose::PreCommit,
    })?;
    if !fresh.store_exists || !fresh.store_regular || !fresh.store_hardlink_free {
        return Err(AppError::UnsafeFilesystemEntry {
            path: store_path.clone(),
            reason: "repair store entry changed before commit",
        });
    }
    if fresh.repo_entry_kind == RepoEntryKind::RegularFile {
        return Err(AppError::PathIsRegularFile {
            path: abs_path.clone(),
        });
    }
    let materialization_action = match fresh.repo_entry_kind {
        RepoEntryKind::Missing => MaterializationAction::Create {
            location: location.clone(),
            strategy,
        },
        RepoEntryKind::UnmanagedSymlinkOrReparsePoint if force => MaterializationAction::Replace {
            location: location.clone(),
            strategy,
            expected: fresh.expected(),
        },
        RepoEntryKind::UnmanagedSymlinkOrReparsePoint => {
            return Err(AppError::RepairSymlinkTargetMismatch {
                path: abs_path.clone(),
                actual_target: fresh.unmanaged_symlink_target.unwrap_or_default(),
                expected_target: store_path.clone(),
            });
        }
        RepoEntryKind::ManagedSymlink => return Ok(()),
        RepoEntryKind::Directory | RepoEntryKind::Other => {
            return Err(AppError::PathIsRegularFile {
                path: abs_path.clone(),
            });
        }
        RepoEntryKind::RegularFile => unreachable!(),
    };

    // A failpoint here lets tests mutate the target ignore state between the
    // operation check and the journal's authorization check.
    failpoint::after(Failpoint::WritePreconditionsValidated)?;
    let mut journal = RepairMutationJournal::new(
        &ctx.config.store,
        &ctx.repo_root,
        ignore,
        ctx.repo_id.clone(),
        path.clone(),
        abs_path.clone(),
        store_path.clone(),
    );
    let prepared = materializer.prepare(materialization_action, &mut journal)?;
    let facts = materializer.inspect(MaterializationInspectionRequest {
        location: location.clone(),
        purpose: InspectionPurpose::PreCommit,
    })?;
    if facts.repo_entry_kind == RepoEntryKind::RegularFile
        || !facts.store_exists
        || !facts.store_regular
        || !facts.store_hardlink_free
    {
        return Err(AppError::FilesystemEntryChanged {
            path: abs_path.clone(),
        });
    }
    let permit =
        journal.issue_commit_permit(facts.write_precondition_guard(prepared.commit_context()))?;
    materializer.commit(prepared, permit)?;

    let post = materializer.inspect(MaterializationInspectionRequest {
        location,
        purpose: InspectionPurpose::PostCommit,
    })?;
    let materialized = match strategy {
        MaterializationStrategy::Symlink => post.repo_entry_kind == RepoEntryKind::ManagedSymlink,
        MaterializationStrategy::Copy => {
            post.repo_entry_kind == RepoEntryKind::RegularFile
                && post.copy_content == CopyContentState::Equal
                && post.hardlink_free
        }
    };
    if !materialized
        || !ignore.has_entry(&ctx.repo_root, path)?
        || git::is_tracked(&ctx.repo_root, abs_path)?
    {
        return Err(AppError::Internal(
            "repair postconditions failed; materialization was retained for inspection".into(),
        ));
    }
    journal.cleanup_all()
}

fn repair_outcome(action: &RepoRepairSymlinkAction) -> RepairOutcome {
    match action {
        RepoRepairSymlinkAction::Recreate { .. } | RepoRepairSymlinkAction::CreateCopy { .. } => {
            RepairOutcome::LinkRecreated
        }
        RepoRepairSymlinkAction::AlreadyHealthy { .. } => RepairOutcome::AlreadyHealthy,
        RepoRepairSymlinkAction::StoreMissing { .. } => RepairOutcome::StoreMissing,
        RepoRepairSymlinkAction::CopyDiverged { .. } => RepairOutcome::CopyDiverged,
        RepoRepairSymlinkAction::DetachedDisabled { .. } => RepairOutcome::DetachedDisabled,
        RepoRepairSymlinkAction::NotManaged { .. } | RepoRepairSymlinkAction::Failed { .. } => {
            RepairOutcome::NotManaged
        }
    }
}

fn build_repo_repair_plan(
    ctx: &RepoContext,
    force: bool,
    current: &context::CurrentGitContext,
    idx: &index::Index,
    ignore: &GitInfoExclude,
) -> Result<RepoRepairPlan> {
    // This is deliberately the first exclude observation. A malformed managed
    // block aborts planning before a symlink, index, or manifest is changed.
    let existing: BTreeSet<String> = ignore
        .managed_entries(&ctx.repo_root)?
        .into_iter()
        .collect();
    let exclude_paths =
        repair_policy::repo_repair_exclude_paths(&ctx.manifest, &existing, |repo_id| {
            idx.get(repo_id).is_some()
        });
    let desired: BTreeSet<String> = exclude_paths.iter().cloned().collect();

    let mut symlink_actions = Vec::new();
    for item in ctx
        .manifest
        .items
        .iter()
        .filter(|item| item.ownership_state == OwnershipState::Attached)
    {
        let abs_path = ctx.repo_root.join(&item.path);
        match build_repair_action_for_item(
            ctx,
            item,
            &abs_path,
            force,
            desired.contains(&item.path),
        ) {
            Ok(action) => symlink_actions.push(action),
            Err(err) if required_repair_inspection_failed(&err) => return Err(err),
            Err(err) => symlink_actions.push(RepoRepairSymlinkAction::Failed {
                path: item.path.clone(),
                reason: err.to_string(),
            }),
        }
    }

    let exclude_updated = existing != desired;
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
        exclude_paths,
        exclude_updated,
        index_updated,
        hints_updated,
    })
}

/// Errors which mean the required repo/store entry could not be inspected
/// safely. Repo repair must not rewrite the managed exclude block in this
/// situation, because that would make a partially observed repository look
/// repaired.
fn required_repair_inspection_failed(error: &AppError) -> bool {
    matches!(
        error,
        AppError::Io { .. }
            | AppError::UnsafeFilesystemEntry { .. }
            | AppError::HardlinkedFile { .. }
            | AppError::FilesystemEntryChanged { .. }
            | AppError::FilesystemCapabilityUnavailable { .. }
    )
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
/// associated with `ctx.repo_id`. The desired target exclude set is durably
/// written and verified before any materialization is changed.
pub fn repair_repo<L: ?Sized>(
    ctx: &mut RepoContext,
    _legacy_link: &L,
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

    let ignore = GitInfoExclude;
    let plan = build_repo_repair_plan(ctx, force, &current, &idx, &ignore)?;
    let mut report = RepairRepoReport::from_plan(plan);
    if dry_run {
        return Ok(report);
    }

    if report.plan.exclude_updated {
        ignore.update_entries(&ctx.repo_root, report.plan.exclude_paths.clone())?;
        if !ignore.entries_match(&ctx.repo_root, report.plan.exclude_paths.clone())? {
            return Err(AppError::Internal(
                "repo repair exclude desired set was not persisted".into(),
            ));
        }
        failpoint::after(Failpoint::RepoRepairTargetExcludeUpdated)?;
    }

    for action in &report.plan.symlink_actions {
        if !matches!(
            action,
            RepoRepairSymlinkAction::Recreate { .. } | RepoRepairSymlinkAction::CreateCopy { .. }
        ) {
            continue;
        }
        if let Err(err) = execute_repair_action(ctx, action, &ignore, force) {
            report.symlinks_repaired = report.symlinks_repaired.saturating_sub(1);
            report
                .symlinks_failed
                .push((action.path().to_string(), err.to_string()));
        }
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
