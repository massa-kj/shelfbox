//! Copy-aware durable item move.
//!
//! The canonical file moves first through `CanonicalTransfer`; repository
//! materialization is then recreated with the strategy that was actually
//! observed at the old path.  Configuration is deliberately not consulted
//! for this choice, because a repository may predate a future default change.

use std::path::Path;

use ulid::Ulid;

use crate::{
    context::{self, RepoContext},
    domain::{
        materialization::{CopyContentState, MaterializationStrategy},
        operation_record::{
            OperationKind, OperationPhase, OperationPostState, OperationPreState, OperationRecord,
            RecoveryAbsolutePath, RecoveryRecord, RecoveryRecordKind,
            OPERATION_RECORD_SCHEMA_VERSION,
        },
        path::{RepoRelativePath, StoreRelativePath},
        recovery_fingerprint::RecoveryFingerprint,
    },
    error::{AppError, Result},
    fs::{
        canonical_transfer::{
            CanonicalEntryKind, CanonicalInspectionPurpose, CanonicalTransfer,
            CanonicalTransferAction, CanonicalTransferInspectionRequest, DefaultCanonicalTransfer,
            ExpectedCanonicalEntry,
        },
        materializer::{
            DefaultMaterializer, InspectionPurpose, MaterializationAction,
            MaterializationInspectionRequest, MaterializationLocation, Materializer,
            MutationJournal, RepoEntryKind,
        },
        mutation_journal::AddMutationJournal,
    },
    git,
    ignore::IgnoreBackend,
    link::LinkStrategy,
    plan::item_move::{ItemMovePlan, ItemMoveReport, ItemMoveWarning},
    storage::operation_record_store,
    store::manifest,
};

pub fn move_item(
    ctx: &mut RepoContext,
    old_abs: &Path,
    new_abs: &Path,
    dry_run: bool,
    _link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<ItemMoveReport> {
    let plan = ItemMovePlan::build(ctx, old_abs, new_abs)?;
    if dry_run {
        return Ok(ItemMoveReport {
            plan,
            dry_run: true,
            warnings: Vec::new(),
        });
    }
    execute_move(ctx, &plan, ignore)?;
    Ok(ItemMoveReport {
        plan,
        dry_run: false,
        warnings: Vec::new(),
    })
}

fn execute_move(
    ctx: &mut RepoContext,
    plan: &ItemMovePlan,
    ignore: &dyn IgnoreBackend,
) -> Result<()> {
    let old_repo = repo_path(&plan.old_path, &plan.old_abs_path)?;
    let new_repo = repo_path(&plan.new_path, &plan.new_abs_path)?;
    let old_store = store_relative(&ctx.config.store, &plan.old_store_path)?;
    let new_store = store_relative(&ctx.config.store, &plan.new_store_path)?;
    let old_location = MaterializationLocation::new(old_repo.clone(), old_store.clone());
    let new_location = MaterializationLocation::new(new_repo.clone(), new_store.clone());
    let mut materializer =
        DefaultMaterializer::new(ctx.repo_root.clone(), ctx.config.store.clone());
    let old_facts = materializer.inspect(MaterializationInspectionRequest {
        location: old_location.clone(),
        purpose: InspectionPurpose::Planning,
    })?;
    let strategy = observed_strategy(&old_facts, &plan.old_abs_path)?;
    let store_fingerprint = RecoveryFingerprint::from_file(&plan.old_store_path)?;
    if git::is_tracked(&ctx.repo_root, &plan.old_abs_path)? {
        return Err(AppError::PathIsTracked {
            path: plan.old_abs_path.clone(),
        });
    }
    if !ignore.has_entry(&ctx.repo_root, &plan.old_path)? {
        return Err(AppError::Internal(
            "managed move source exclude is missing; refusing to move an unexcluded path".into(),
        ));
    }
    if plan.new_abs_path.symlink_metadata().is_ok() {
        return Err(AppError::MoveDestinationExists {
            path: plan.new_abs_path.clone(),
        });
    }
    if git::is_tracked(&ctx.repo_root, &plan.new_abs_path)? {
        return Err(AppError::PathIsTracked {
            path: plan.new_abs_path.clone(),
        });
    }

    let mut record = RecoveryRecord {
        schema_version: OPERATION_RECORD_SCHEMA_VERSION,
        record_id: Ulid::new().to_string(),
        created_at: context::now_iso8601(),
        record: RecoveryRecordKind::Operation(OperationRecord {
            operation: OperationKind::Move,
            phase: OperationPhase::RecordCreated,
            repo_id: ctx.repo_id.clone(),
            repo_root: RecoveryAbsolutePath::new(&ctx.repo_root).ok_or_else(|| {
                AppError::UnsafeFilesystemEntry {
                    path: ctx.repo_root.clone(),
                    reason: "repository root is not a safe absolute path",
                }
            })?,
            repo_store_path: Some(store_relative(&ctx.config.store, &ctx.repo_store)?),
            strategy,
            direction: None,
            pre_state: OperationPreState {
                repo_path: Some(old_repo.clone()),
                store_path: Some(old_store.clone()),
                repo_fingerprint: None,
                store_fingerprint: Some(store_fingerprint),
                manifest_contains_item: Some(true),
                exclude_owned: Some(true),
                final_exclude_owned: Some(false),
            },
            post_state: Some(OperationPostState {
                repo_path: Some(new_repo.clone()),
                store_path: Some(new_store.clone()),
                manifest_contains_item: Some(true),
                exclude_owned: Some(true),
            }),
            artifact_record_ids: Vec::new(),
            backup: None,
        }),
    };
    let record_id = record.record_id.clone();
    operation_record_store::create(&ctx.config.store, &record)?;
    let store_root = ctx.config.store.clone();
    let repo_root = ctx.repo_root.clone();
    let mut journal = AddMutationJournal::new(
        &store_root,
        &repo_root,
        ignore,
        &mut record,
        plan.new_abs_path.clone(),
        plan.new_store_path.clone(),
    );

    // Destination exclusion is durable and verified before a regular Copy can
    // ever appear there.  The old entry remains excluded until finalization.
    ignore.add_entries(&repo_root, &[&plan.new_path])?;
    if !ignore.has_entry(&repo_root, &plan.new_path)? {
        return Err(AppError::Internal(
            "move destination exclude was not persisted".into(),
        ));
    }
    journal.advance(OperationPhase::DestinationExcluded)?;
    journal.ensure_store_destination_parent()?;

    let mut transfer = DefaultCanonicalTransfer::new(repo_root.clone(), store_root.clone());
    let inspection = CanonicalTransferAction::Move {
        source: old_store.clone(),
        destination: new_store.clone(),
        expected_source: ExpectedCanonicalEntry::unchecked(CanonicalEntryKind::RegularFile),
        expected_destination: ExpectedCanonicalEntry::unchecked(CanonicalEntryKind::Missing),
    };
    let planning = transfer.inspect(CanonicalTransferInspectionRequest {
        action: inspection,
        purpose: CanonicalInspectionPurpose::Planning,
    })?;
    ensure_store_move_facts(&planning, &plan.old_store_path, &plan.new_store_path)?;
    let action = CanonicalTransferAction::Move {
        source: old_store,
        destination: new_store,
        expected_source: planning.expected_source(),
        expected_destination: planning.expected_destination(),
    };
    let prepared = transfer.prepare(action.clone(), &mut journal)?;
    let facts = transfer.inspect(CanonicalTransferInspectionRequest {
        action: action.clone(),
        purpose: CanonicalInspectionPurpose::PreCommit,
    })?;
    ensure_store_move_facts(&facts, &plan.old_store_path, &plan.new_store_path)?;
    validate_move_integration(ctx, plan, ignore)?;
    let permit =
        journal.issue_commit_permit(facts.write_precondition_guard(prepared.commit_context()))?;
    transfer.commit(prepared, permit)?;
    journal.advance(OperationPhase::StoreTransferred)?;

    // Preserve both strategy and bytes observed at the old path.  The remove
    // action rejects a replaced old endpoint, and create refuses a raced new
    // endpoint; neither path ever performs delete-then-create replacement.
    let old_now = materializer.inspect(MaterializationInspectionRequest {
        location: old_location.clone(),
        purpose: InspectionPurpose::PreCommit,
    })?;
    if old_now.repo_entry_kind != old_facts.repo_entry_kind {
        return Err(AppError::FilesystemEntryChanged {
            path: plan.old_abs_path.clone(),
        });
    }
    let remove = MaterializationAction::Remove {
        location: old_location,
        expected: old_facts.expected(),
    };
    let prepared_remove = materializer.prepare(remove, &mut journal)?;
    validate_move_integration(ctx, plan, ignore)?;
    let permit = journal
        .issue_commit_permit(old_now.write_precondition_guard(prepared_remove.commit_context()))?;
    materializer.commit(prepared_remove, permit)?;

    let new_before = materializer.inspect(MaterializationInspectionRequest {
        location: new_location.clone(),
        purpose: InspectionPurpose::PreCommit,
    })?;
    if new_before.repo_entry_kind != RepoEntryKind::Missing {
        return Err(AppError::MoveDestinationExists {
            path: plan.new_abs_path.clone(),
        });
    }
    let create = MaterializationAction::Create {
        location: new_location.clone(),
        strategy,
    };
    let prepared_create = materializer.prepare(create, &mut journal)?;
    let new_fresh = materializer.inspect(MaterializationInspectionRequest {
        location: new_location.clone(),
        purpose: InspectionPurpose::PreCommit,
    })?;
    if new_fresh.repo_entry_kind != RepoEntryKind::Missing {
        return Err(AppError::MoveDestinationExists {
            path: plan.new_abs_path.clone(),
        });
    }
    validate_move_integration(ctx, plan, ignore)?;
    let permit = journal.issue_commit_permit(
        new_fresh.write_precondition_guard(prepared_create.commit_context()),
    )?;
    materializer.commit(prepared_create, permit)?;
    journal.advance(OperationPhase::RepoMoved)?;

    let now = context::now_iso8601();
    ctx.manifest.rename(
        &plan.old_path,
        &plan.new_path,
        &plan.new_store_path_relative,
        &now,
    );
    manifest::save(&ctx.repo_store, &ctx.manifest)?;
    journal.advance(OperationPhase::ManifestSaved)?;
    ignore.remove_entries(&repo_root, &[&plan.old_path])?;
    if ignore.has_entry(&repo_root, &plan.old_path)?
        || !ignore.has_entry(&repo_root, &plan.new_path)?
    {
        return Err(AppError::Internal(
            "move exclude finalization failed".into(),
        ));
    }
    journal.advance(OperationPhase::ExcludeFinalized)?;

    let final_facts = materializer.inspect(MaterializationInspectionRequest {
        location: new_location,
        purpose: InspectionPurpose::PostCommit,
    })?;
    if observed_strategy(&final_facts, &plan.new_abs_path)? != strategy
        || !ctx.manifest.contains(&plan.new_path)
        || ctx.manifest.contains(&plan.old_path)
    {
        return Err(AppError::Internal(
            "move postconditions failed; durable recovery record was retained".into(),
        ));
    }
    journal.cleanup_all()?;
    journal.advance(OperationPhase::PostCommitValidated)?;
    drop(journal);
    operation_record_store::remove(&store_root, &record_id)
}

fn validate_move_integration(
    ctx: &RepoContext,
    plan: &ItemMovePlan,
    ignore: &dyn IgnoreBackend,
) -> Result<()> {
    if git::is_tracked(&ctx.repo_root, &plan.old_abs_path)?
        || git::is_tracked(&ctx.repo_root, &plan.new_abs_path)?
    {
        return Err(AppError::PathIsTracked {
            path: plan.new_abs_path.clone(),
        });
    }
    if !ignore.has_entry(&ctx.repo_root, &plan.old_path)?
        || !ignore.has_entry(&ctx.repo_root, &plan.new_path)?
    {
        return Err(AppError::Internal(
            "move excludes changed before commit authorization".into(),
        ));
    }
    Ok(())
}

fn observed_strategy(
    facts: &crate::fs::materializer::MaterializationFacts,
    path: &Path,
) -> Result<MaterializationStrategy> {
    if !facts.store_exists || !facts.store_regular {
        return Err(AppError::StoreMissing {
            path: path.to_path_buf(),
            store_path: path.to_path_buf(),
        });
    }
    if !facts.store_hardlink_free || !facts.hardlink_free {
        return Err(AppError::HardlinkedFile {
            path: path.to_path_buf(),
        });
    }
    match facts.repo_entry_kind {
        RepoEntryKind::ManagedSymlink => Ok(MaterializationStrategy::Symlink),
        RepoEntryKind::RegularFile if facts.copy_content == CopyContentState::Equal => {
            Ok(MaterializationStrategy::Copy)
        }
        RepoEntryKind::RegularFile if facts.copy_content == CopyContentState::Diverged => {
            Err(AppError::ContentDivergedRequiresSync {
                path: path.to_path_buf(),
            })
        }
        RepoEntryKind::UnmanagedSymlinkOrReparsePoint => Err(AppError::MoveSourceSymlinkMismatch {
            path: path.to_path_buf(),
        }),
        _ => Err(AppError::UnsafeFilesystemEntry {
            path: path.to_path_buf(),
            reason: "move source is not an observed managed materialization",
        }),
    }
}

fn ensure_store_move_facts(
    facts: &crate::fs::canonical_transfer::CanonicalTransferFacts,
    source: &Path,
    destination: &Path,
) -> Result<()> {
    if facts.source_kind != CanonicalEntryKind::RegularFile || !facts.source_hardlink_free {
        return Err(AppError::UnsafeFilesystemEntry {
            path: source.to_path_buf(),
            reason: "move canonical source is not an isolated regular file",
        });
    }
    if facts.destination_kind != CanonicalEntryKind::Missing {
        return Err(AppError::MoveDestinationExists {
            path: destination.to_path_buf(),
        });
    }
    Ok(())
}

fn repo_path(value: &str, absolute: &Path) -> Result<RepoRelativePath> {
    RepoRelativePath::new(value.to_owned()).ok_or_else(|| AppError::UnsafeFilesystemEntry {
        path: absolute.to_path_buf(),
        reason: "move repository path is not normalized",
    })
}

fn store_relative(store_root: &Path, absolute: &Path) -> Result<StoreRelativePath> {
    let relative =
        absolute
            .strip_prefix(store_root)
            .map_err(|_| AppError::UnsafeFilesystemEntry {
                path: absolute.to_path_buf(),
                reason: "move store path escapes the configured store root",
            })?;
    StoreRelativePath::new(relative.to_string_lossy().replace('\\', "/")).ok_or_else(|| {
        AppError::UnsafeFilesystemEntry {
            path: absolute.to_path_buf(),
            reason: "move store path is not normalized",
        }
    })
}

// Kept for the public report shape. Exclude mutations are now durable
// transaction boundaries, so a failed rewrite is returned as an error rather
// than a best-effort warning.
#[allow(dead_code)]
fn _warning_shape(_: ItemMoveWarning) {}
