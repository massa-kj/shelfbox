//! Re-attach detached items without assuming a symlink materialization.

use std::path::Path;

use ulid::Ulid;

use crate::{
    context::{self, RepoContext},
    domain::{
        materialization::{CopyContentState, MaterializationStrategy},
        operation_record::{
            ArtifactLocation, OperationDirection, OperationKind, OperationPhase, OperationPreState,
            OperationRecord, RecoveryAbsolutePath, RecoveryBackupMetadata, RecoveryRecord,
            RecoveryRecordKind, OPERATION_RECORD_SCHEMA_VERSION,
        },
        path::{RepoRelativePath, StoreRelativePath},
        recovery_fingerprint::RecoveryFingerprint,
    },
    error::{AppError, Result},
    failpoint::{self, Failpoint},
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
        mutation_journal::{AddMutationJournal, RepairMutationJournal},
    },
    git,
    ignore::{GitInfoExclude, IgnoreBackend},
    link::LinkStrategy,
    plan::item_relink::{ItemRelinkPlan, ItemRelinkReport},
    storage::operation_record_store,
    store::manifest::{self, OwnershipState},
};

use super::path::repo_relative_string;

pub use crate::plan::item_relink::RelinkOutcome;

/// Direction selection for the Phase 10 directional relink API.  The legacy
/// `relink` entry point deliberately remains directionless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelinkDirection {
    FromStore,
    FromRepo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemRelinkRequest {
    pub direction: Option<RelinkDirection>,
    pub dry_run: bool,
    pub confirmed: bool,
}

pub(crate) fn relink_report(
    ctx: &mut RepoContext,
    abs_path: &Path,
    dry_run: bool,
    _link: &dyn LinkStrategy,
) -> Result<ItemRelinkReport> {
    relink_report_with_request(
        ctx,
        abs_path,
        ItemRelinkRequest {
            direction: None,
            dry_run,
            confirmed: false,
        },
        &GitInfoExclude,
    )
}

pub(crate) fn relink_report_with_request(
    ctx: &mut RepoContext,
    abs_path: &Path,
    request: ItemRelinkRequest,
    ignore: &dyn IgnoreBackend,
) -> Result<ItemRelinkReport> {
    let plan = match request.direction {
        Some(_) => build_directional_relink_plan(ctx, abs_path)?,
        None => build_relink_plan(ctx, abs_path)?,
    };
    if request.dry_run {
        return Ok(ItemRelinkReport {
            plan,
            outcome: RelinkOutcome::WouldRelink,
            dry_run: true,
        });
    }
    let outcome = match request.direction {
        Some(direction) => {
            if direction == RelinkDirection::FromRepo && !request.confirmed {
                return Err(AppError::SyncConfirmationRequired);
            }
            execute_directional_relink(ctx, &plan, direction, ignore)?
        }
        None => execute_directionless_relink(ctx, &plan, ignore)?,
    };
    Ok(ItemRelinkReport {
        plan,
        outcome,
        dry_run: false,
    })
}

fn build_directional_relink_plan(ctx: &RepoContext, abs_path: &Path) -> Result<ItemRelinkPlan> {
    let path = repo_relative_string(&ctx.repo_root, abs_path)?;
    let item = ctx
        .manifest
        .get(&path)
        .ok_or_else(|| AppError::NotManagedLink {
            path: abs_path.to_path_buf(),
        })?;
    if item.ownership_state != OwnershipState::Detached {
        return Err(AppError::RelinkNotDetached {
            path: abs_path.to_path_buf(),
            actual_state: format!("{:?}", item.ownership_state),
        });
    }
    let location = materialization_location(ctx, &path, &item.store_path)?;
    let materializer = DefaultMaterializer::new(ctx.repo_root.clone(), ctx.config.store.clone());
    let facts = materializer.inspect(MaterializationInspectionRequest {
        location,
        purpose: InspectionPurpose::Planning,
    })?;
    if facts.repo_entry_kind != RepoEntryKind::RegularFile
        || !facts.hardlink_free
        || !facts.store_exists
        || !facts.store_regular
        || !facts.store_hardlink_free
    {
        return Err(AppError::SyncRequiresRegularCopy {
            path: abs_path.to_path_buf(),
        });
    }
    if facts.copy_content != CopyContentState::Diverged {
        return Err(AppError::ContentDivergedRequiresSync {
            path: abs_path.to_path_buf(),
        });
    }
    Ok(ItemRelinkPlan {
        path,
        abs_path: abs_path.to_path_buf(),
        store_path: ctx.repo_store.join(&item.store_path),
        symlink_ok: false,
    })
}

fn build_relink_plan(ctx: &RepoContext, abs_path: &Path) -> Result<ItemRelinkPlan> {
    let path = repo_relative_string(&ctx.repo_root, abs_path)?;
    let item = ctx
        .manifest
        .get(&path)
        .ok_or_else(|| AppError::NotManagedLink {
            path: abs_path.to_path_buf(),
        })?;
    let store_path = ctx.repo_store.join(&item.store_path);
    if !store_path.exists() {
        return Err(AppError::StoreMissing {
            path: abs_path.to_path_buf(),
            store_path,
        });
    }
    let location = materialization_location(ctx, &path, &item.store_path)?;
    let materializer = DefaultMaterializer::new(ctx.repo_root.clone(), ctx.config.store.clone());
    let facts = materializer.inspect(MaterializationInspectionRequest {
        location,
        purpose: InspectionPurpose::Planning,
    })?;
    if !facts.store_regular || !facts.store_hardlink_free {
        return Err(AppError::UnsafeFilesystemEntry {
            path: abs_path.to_path_buf(),
            reason: "relink canonical store entry is not an isolated regular file",
        });
    }
    let healthy = is_healthy(&facts);
    if item.ownership_state != OwnershipState::Detached && !healthy {
        return Err(AppError::RelinkNotDetached {
            path: abs_path.to_path_buf(),
            actual_state: format!("{:?}", item.ownership_state),
        });
    }
    if facts.repo_entry_kind == RepoEntryKind::RegularFile
        && facts.copy_content == CopyContentState::Diverged
    {
        return Err(AppError::ContentDivergedRequiresSync {
            path: abs_path.to_path_buf(),
        });
    }
    if !matches!(facts.repo_entry_kind, RepoEntryKind::Missing) && !healthy {
        return Err(AppError::NotManagedLink {
            path: abs_path.to_path_buf(),
        });
    }
    Ok(ItemRelinkPlan {
        path,
        abs_path: abs_path.to_path_buf(),
        store_path: ctx.repo_store.join(&item.store_path),
        // Kept for API compatibility.  It now means any healthy observed
        // materialization, not only a managed symlink.
        symlink_ok: healthy,
    })
}

fn execute_directionless_relink(
    ctx: &mut RepoContext,
    plan: &ItemRelinkPlan,
    ignore: &dyn IgnoreBackend,
) -> Result<RelinkOutcome> {
    let item = ctx.manifest.get(&plan.path).ok_or_else(|| {
        AppError::Internal("relink plan item disappeared from the manifest".into())
    })?;
    let location = materialization_location(ctx, &plan.path, &item.store_path)?;
    let mut materializer =
        DefaultMaterializer::new(ctx.repo_root.clone(), ctx.config.store.clone());
    let facts = materializer.inspect(MaterializationInspectionRequest {
        location: location.clone(),
        purpose: InspectionPurpose::Planning,
    })?;
    if git::is_tracked(&ctx.repo_root, &plan.abs_path)? {
        return Err(AppError::PathIsTracked {
            path: plan.abs_path.clone(),
        });
    }
    // The exact target entry is established before any Copy artifact is
    // leased.  A retry after an interruption sees the same entry and simply
    // continues; no durable lifecycle record is needed for directionless
    // reattachment.
    ignore.add_entries(&ctx.repo_root, &[&plan.path])?;
    if !ignore.has_entry(&ctx.repo_root, &plan.path)? {
        return Err(AppError::Internal(
            "relink exclude was not persisted".into(),
        ));
    }

    let outcome = if is_healthy(&facts) {
        RelinkOutcome::StateUpdated
    } else if facts.repo_entry_kind == RepoEntryKind::Missing {
        let mut journal = RepairMutationJournal::new(
            &ctx.config.store,
            &ctx.repo_root,
            ignore,
            ctx.repo_id.clone(),
            plan.path.clone(),
            plan.abs_path.clone(),
            plan.store_path.clone(),
        )
        .with_durability(ctx.config.mutation_durability);
        let action = MaterializationAction::Create {
            location: location.clone(),
            strategy: ctx.config.materialization,
        };
        let prepared = materializer.prepare(action, &mut journal)?;
        let fresh = materializer.inspect(MaterializationInspectionRequest {
            location: location.clone(),
            purpose: InspectionPurpose::PreCommit,
        })?;
        if fresh.repo_entry_kind != RepoEntryKind::Missing
            || git::is_tracked(&ctx.repo_root, &plan.abs_path)?
        {
            return Err(AppError::FilesystemEntryChanged {
                path: plan.abs_path.clone(),
            });
        }
        let permit = journal
            .issue_commit_permit(fresh.write_precondition_guard(prepared.commit_context()))?;
        materializer.commit(prepared, permit)?;
        failpoint::after(Failpoint::DirectionlessRelinkMaterialized)?;
        let post = materializer.inspect(MaterializationInspectionRequest {
            location: location.clone(),
            purpose: InspectionPurpose::PostCommit,
        })?;
        if !is_healthy(&post) {
            return Err(AppError::Internal(
                "relink materialization postcondition failed; artifact record was retained".into(),
            ));
        }
        journal.cleanup_all()?;
        RelinkOutcome::Relinked
    } else {
        return Err(AppError::NotManagedLink {
            path: plan.abs_path.clone(),
        });
    };

    let now = context::now_iso8601();
    ctx.manifest
        .set_ownership_state(&plan.path, OwnershipState::Attached, &now);
    manifest::save(&ctx.repo_store, &ctx.manifest)?;
    failpoint::after(Failpoint::DirectionlessRelinkManifestSaved)?;
    if !ignore.has_entry(&ctx.repo_root, &plan.path)?
        || git::is_tracked(&ctx.repo_root, &plan.abs_path)?
    {
        return Err(AppError::Internal(
            "relink postcondition failed after manifest save".into(),
        ));
    }
    Ok(outcome)
}

/// Explicitly resolves a diverged detached Copy.  The losing endpoint is
/// retained in a store-side recovery backup until attachment is durable; a
/// replacement or cleanup conflict therefore preserves both byte streams.
fn execute_directional_relink(
    ctx: &mut RepoContext,
    plan: &ItemRelinkPlan,
    direction: RelinkDirection,
    ignore: &dyn IgnoreBackend,
) -> Result<RelinkOutcome> {
    let _item = ctx.manifest.get(&plan.path).ok_or_else(|| {
        AppError::Internal("directional relink plan item disappeared from the manifest".into())
    })?;
    let repo_path = RepoRelativePath::new(plan.path.clone()).ok_or_else(|| {
        AppError::UnsafeFilesystemEntry {
            path: plan.abs_path.clone(),
            reason: "relink repository path is not normalized",
        }
    })?;
    let store_path = store_relative_path(&ctx.config.store, &plan.store_path)?;
    let location = MaterializationLocation::new(repo_path.clone(), store_path.clone());
    let mut materializer =
        DefaultMaterializer::new(ctx.repo_root.clone(), ctx.config.store.clone());
    let initial = materializer.inspect(MaterializationInspectionRequest {
        location: location.clone(),
        purpose: InspectionPurpose::Planning,
    })?;
    if initial.repo_entry_kind != RepoEntryKind::RegularFile
        || !initial.hardlink_free
        || !initial.store_exists
        || !initial.store_regular
        || !initial.store_hardlink_free
        || initial.copy_content != CopyContentState::Diverged
    {
        return Err(AppError::ContentDivergedRequiresSync {
            path: plan.abs_path.clone(),
        });
    }
    if git::is_tracked(&ctx.repo_root, &plan.abs_path)? {
        return Err(AppError::PathIsTracked {
            path: plan.abs_path.clone(),
        });
    }
    let repo_before = RecoveryFingerprint::from_file(&plan.abs_path)?;
    let store_before = RecoveryFingerprint::from_file(&plan.store_path)?;
    let record_id = Ulid::new().to_string();
    let backup_store_path = StoreRelativePath::new(format!("recovery/{record_id}.relink"))
        .ok_or_else(|| {
            AppError::Internal("generated relink backup path was not normalized".into())
        })?;
    let backup_path = ctx.config.store.join(backup_store_path.as_str());
    let backup = RecoveryBackupMetadata {
        artifact_record_id: Ulid::new().to_string(),
        location: ArtifactLocation::Store {
            path: backup_store_path.clone(),
        },
        expected_identity: None,
        fingerprint: None,
    };
    let mut record = RecoveryRecord {
        schema_version: OPERATION_RECORD_SCHEMA_VERSION,
        durability: ctx.config.mutation_durability,
        record_id: record_id.clone(),
        created_at: context::now_iso8601(),
        record: RecoveryRecordKind::Operation(OperationRecord {
            operation: OperationKind::Relink,
            phase: OperationPhase::RecordCreated,
            repo_id: ctx.repo_id.clone(),
            repo_root: RecoveryAbsolutePath::new(&ctx.repo_root).ok_or_else(|| {
                AppError::UnsafeFilesystemEntry {
                    path: ctx.repo_root.clone(),
                    reason: "repository root is not a safe absolute path",
                }
            })?,
            repo_store_path: Some(store_relative_path(&ctx.config.store, &ctx.repo_store)?),
            strategy: MaterializationStrategy::Copy,
            direction: Some(match direction {
                RelinkDirection::FromStore => OperationDirection::FromStore,
                RelinkDirection::FromRepo => OperationDirection::FromRepo,
            }),
            pre_state: OperationPreState {
                repo_path: Some(repo_path.clone()),
                store_path: Some(store_path.clone()),
                repo_fingerprint: Some(repo_before.clone()),
                store_fingerprint: Some(store_before.clone()),
                manifest_contains_item: Some(true),
                exclude_owned: Some(ignore.has_entry(&ctx.repo_root, &plan.path)?),
                final_exclude_owned: Some(true),
            },
            post_state: None,
            artifact_record_ids: Vec::new(),
            backup: Some(backup),
        }),
    };
    operation_record_store::create(&ctx.config.store, &record)?;
    let store_root = ctx.config.store.clone();
    let repo_root = ctx.repo_root.clone();

    ignore.add_entries(&repo_root, &[&plan.path])?;
    if !ignore.has_entry(&repo_root, &plan.path)? {
        return Err(AppError::Internal(
            "directional relink exclude was not persisted".into(),
        ));
    }
    update_record_phase(&store_root, &mut record, OperationPhase::ExcludeWritten)?;

    let recorded_backup = {
        let mut stage = DirectionalPreimageStage {
            store_root: &store_root,
            repo_root: &repo_root,
            ctx,
            record: &mut record,
            ignore,
            plan,
        };
        match direction {
            RelinkDirection::FromStore => {
                stage_repo_preimage(&mut stage, &repo_path, &backup_store_path, &backup_path)?
            }
            RelinkDirection::FromRepo => {
                stage_store_preimage(&mut stage, &store_path, &backup_store_path, &backup_path)?
            }
        }
    };

    match direction {
        RelinkDirection::FromStore => {
            let mut journal = AddMutationJournal::new(
                &store_root,
                &repo_root,
                ignore,
                &mut record,
                plan.abs_path.clone(),
                plan.store_path.clone(),
            );
            let facts = materializer.inspect(MaterializationInspectionRequest {
                location: location.clone(),
                purpose: InspectionPurpose::PreCommit,
            })?;
            ensure_diverged_copy(&facts, &plan.abs_path)?;
            let action = MaterializationAction::Replace {
                location: location.clone(),
                strategy: MaterializationStrategy::Copy,
                expected: facts.expected(),
            };
            let prepared = materializer.prepare(action, &mut journal)?;
            let fresh = materializer.inspect(MaterializationInspectionRequest {
                location: location.clone(),
                purpose: InspectionPurpose::PreCommit,
            })?;
            ensure_diverged_copy(&fresh, &plan.abs_path)?;
            validate_directional_integration(ctx, plan, ignore)?;
            let permit = journal
                .issue_commit_permit(fresh.write_precondition_guard(prepared.commit_context()))?;
            materializer.commit(prepared, permit)?;
            journal.cleanup_all()?;
        }
        RelinkDirection::FromRepo => {
            let mut journal = AddMutationJournal::new(
                &store_root,
                &repo_root,
                ignore,
                &mut record,
                plan.abs_path.clone(),
                plan.store_path.clone(),
            );
            let mut transfer = DefaultCanonicalTransfer::new(repo_root.clone(), store_root.clone());
            let inspection = CanonicalTransferAction::CopyFromRepo {
                source: repo_path.clone(),
                destination: store_path.clone(),
                expected_source: ExpectedCanonicalEntry::unchecked(CanonicalEntryKind::RegularFile),
                expected_destination: ExpectedCanonicalEntry::unchecked(
                    CanonicalEntryKind::Missing,
                ),
            };
            let planning = transfer.inspect(CanonicalTransferInspectionRequest {
                action: inspection,
                purpose: CanonicalInspectionPurpose::Planning,
            })?;
            // After staging canonical content, the final canonical path must
            // be absent and the repo preimage still must be a regular Copy.
            if planning.source_kind != CanonicalEntryKind::RegularFile
                || !planning.source_hardlink_free
                || planning.destination_kind != CanonicalEntryKind::Missing
            {
                return Err(AppError::FilesystemEntryChanged {
                    path: plan.store_path.clone(),
                });
            }
            let action = CanonicalTransferAction::CopyFromRepo {
                source: repo_path.clone(),
                destination: store_path.clone(),
                expected_source: planning.expected_source(),
                expected_destination: planning.expected_destination(),
            };
            let prepared = transfer.prepare(action.clone(), &mut journal)?;
            let fresh = transfer.inspect(CanonicalTransferInspectionRequest {
                action: action.clone(),
                purpose: CanonicalInspectionPurpose::PreCommit,
            })?;
            if fresh.source_kind != CanonicalEntryKind::RegularFile
                || fresh.destination_kind != CanonicalEntryKind::Missing
            {
                return Err(AppError::FilesystemEntryChanged {
                    path: plan.store_path.clone(),
                });
            }
            validate_directional_integration(ctx, plan, ignore)?;
            let permit = journal
                .issue_commit_permit(fresh.write_precondition_guard(prepared.commit_context()))?;
            transfer.commit(prepared, permit)?;
            journal.cleanup_all()?;
        }
    }
    update_record_phase(
        &store_root,
        &mut record,
        OperationPhase::ContentSynchronized,
    )?;

    let post = materializer.inspect(MaterializationInspectionRequest {
        location: location.clone(),
        purpose: InspectionPurpose::PostCommit,
    })?;
    if !is_healthy(&post) {
        return Err(AppError::Internal(
            "directional relink synchronization postcondition failed".into(),
        ));
    }
    let canonical_expected = match direction {
        RelinkDirection::FromStore => &store_before,
        RelinkDirection::FromRepo => &repo_before,
    };
    if RecoveryFingerprint::from_file(&plan.abs_path)? != *canonical_expected
        || RecoveryFingerprint::from_file(&plan.store_path)? != *canonical_expected
    {
        return Err(AppError::FilesystemEntryChanged {
            path: plan.abs_path.clone(),
        });
    }

    let now = context::now_iso8601();
    ctx.manifest
        .set_ownership_state(&plan.path, OwnershipState::Attached, &now);
    manifest::save(&ctx.repo_store, &ctx.manifest)?;
    update_record_phase(&store_root, &mut record, OperationPhase::OwnershipAttached)?;
    operation_record_store::cleanup_backup(&store_root, &repo_root, &record_id, &recorded_backup)?;
    update_record_phase(
        &store_root,
        &mut record,
        OperationPhase::PostCommitValidated,
    )?;
    operation_record_store::remove(&store_root, &record_id)?;
    Ok(RelinkOutcome::Relinked)
}

struct DirectionalPreimageStage<'a> {
    store_root: &'a Path,
    repo_root: &'a Path,
    ctx: &'a RepoContext,
    record: &'a mut RecoveryRecord,
    ignore: &'a dyn IgnoreBackend,
    plan: &'a ItemRelinkPlan,
}

fn stage_repo_preimage(
    stage: &mut DirectionalPreimageStage<'_>,
    repo_path: &RepoRelativePath,
    backup_store_path: &StoreRelativePath,
    backup_path: &Path,
) -> Result<RecoveryBackupMetadata> {
    let mut journal = AddMutationJournal::new(
        stage.store_root,
        stage.repo_root,
        stage.ignore,
        stage.record,
        stage.plan.abs_path.clone(),
        backup_path.to_path_buf(),
    );
    journal.ensure_store_destination_parent()?;
    let mut transfer = DefaultCanonicalTransfer::new(
        stage.repo_root.to_path_buf(),
        stage.store_root.to_path_buf(),
    );
    let inspection = CanonicalTransferAction::CopyFromRepo {
        source: repo_path.clone(),
        destination: backup_store_path.clone(),
        expected_source: ExpectedCanonicalEntry::unchecked(CanonicalEntryKind::RegularFile),
        expected_destination: ExpectedCanonicalEntry::unchecked(CanonicalEntryKind::Missing),
    };
    let planning = transfer.inspect(CanonicalTransferInspectionRequest {
        action: inspection,
        purpose: CanonicalInspectionPurpose::Planning,
    })?;
    if planning.source_kind != CanonicalEntryKind::RegularFile
        || !planning.source_hardlink_free
        || planning.destination_kind != CanonicalEntryKind::Missing
    {
        return Err(AppError::FilesystemEntryChanged {
            path: stage.plan.abs_path.clone(),
        });
    }
    let action = CanonicalTransferAction::CopyFromRepo {
        source: repo_path.clone(),
        destination: backup_store_path.clone(),
        expected_source: planning.expected_source(),
        expected_destination: planning.expected_destination(),
    };
    let prepared = transfer.prepare(action.clone(), &mut journal)?;
    let fresh = transfer.inspect(CanonicalTransferInspectionRequest {
        action,
        purpose: CanonicalInspectionPurpose::PreCommit,
    })?;
    if fresh.source_kind != CanonicalEntryKind::RegularFile
        || fresh.destination_kind != CanonicalEntryKind::Missing
    {
        return Err(AppError::FilesystemEntryChanged {
            path: stage.plan.abs_path.clone(),
        });
    }
    validate_directional_integration(stage.ctx, stage.plan, stage.ignore)?;
    let permit =
        journal.issue_commit_permit(fresh.write_precondition_guard(prepared.commit_context()))?;
    transfer.commit(prepared, permit)?;
    let backup = journal.record_backup_from_path(backup_path)?;
    journal.cleanup_all()?;
    Ok(backup)
}

fn stage_store_preimage(
    stage: &mut DirectionalPreimageStage<'_>,
    store_path: &StoreRelativePath,
    backup_store_path: &StoreRelativePath,
    backup_path: &Path,
) -> Result<RecoveryBackupMetadata> {
    let mut journal = AddMutationJournal::new(
        stage.store_root,
        stage.repo_root,
        stage.ignore,
        stage.record,
        stage.plan.abs_path.clone(),
        backup_path.to_path_buf(),
    );
    journal.ensure_store_destination_parent()?;
    let mut transfer = DefaultCanonicalTransfer::new(
        stage.repo_root.to_path_buf(),
        stage.store_root.to_path_buf(),
    );
    let inspection = CanonicalTransferAction::Move {
        source: store_path.clone(),
        destination: backup_store_path.clone(),
        expected_source: ExpectedCanonicalEntry::unchecked(CanonicalEntryKind::RegularFile),
        expected_destination: ExpectedCanonicalEntry::unchecked(CanonicalEntryKind::Missing),
    };
    let planning = transfer.inspect(CanonicalTransferInspectionRequest {
        action: inspection,
        purpose: CanonicalInspectionPurpose::Planning,
    })?;
    if planning.source_kind != CanonicalEntryKind::RegularFile
        || !planning.source_hardlink_free
        || planning.destination_kind != CanonicalEntryKind::Missing
    {
        return Err(AppError::FilesystemEntryChanged {
            path: stage.plan.store_path.clone(),
        });
    }
    let action = CanonicalTransferAction::Move {
        source: store_path.clone(),
        destination: backup_store_path.clone(),
        expected_source: planning.expected_source(),
        expected_destination: planning.expected_destination(),
    };
    let prepared = transfer.prepare(action.clone(), &mut journal)?;
    let fresh = transfer.inspect(CanonicalTransferInspectionRequest {
        action,
        purpose: CanonicalInspectionPurpose::PreCommit,
    })?;
    if fresh.source_kind != CanonicalEntryKind::RegularFile
        || fresh.destination_kind != CanonicalEntryKind::Missing
    {
        return Err(AppError::FilesystemEntryChanged {
            path: stage.plan.store_path.clone(),
        });
    }
    validate_directional_integration(stage.ctx, stage.plan, stage.ignore)?;
    let permit =
        journal.issue_commit_permit(fresh.write_precondition_guard(prepared.commit_context()))?;
    transfer.commit(prepared, permit)?;
    let backup = journal.record_backup_from_path(backup_path)?;
    journal.cleanup_all()?;
    Ok(backup)
}

fn ensure_diverged_copy(
    facts: &crate::fs::materializer::MaterializationFacts,
    path: &Path,
) -> Result<()> {
    if facts.repo_entry_kind != RepoEntryKind::RegularFile
        || !facts.hardlink_free
        || !facts.store_exists
        || !facts.store_regular
        || !facts.store_hardlink_free
        || facts.copy_content != CopyContentState::Diverged
    {
        return Err(AppError::FilesystemEntryChanged {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

fn validate_directional_integration(
    ctx: &RepoContext,
    plan: &ItemRelinkPlan,
    ignore: &dyn IgnoreBackend,
) -> Result<()> {
    if git::is_tracked(&ctx.repo_root, &plan.abs_path)? {
        return Err(AppError::PathIsTracked {
            path: plan.abs_path.clone(),
        });
    }
    if !ignore.has_entry(&ctx.repo_root, &plan.path)? {
        return Err(AppError::Internal(
            "directional relink exclude changed before commit".into(),
        ));
    }
    Ok(())
}

fn update_record_phase(
    store_root: &Path,
    record: &mut RecoveryRecord,
    phase: OperationPhase,
) -> Result<()> {
    let RecoveryRecordKind::Operation(operation) = &mut record.record else {
        unreachable!()
    };
    operation.phase = phase;
    operation_record_store::update(store_root, record)
}

fn materialization_location(
    ctx: &RepoContext,
    repo_path: &str,
    item_store_path: &str,
) -> Result<MaterializationLocation> {
    let repo_path = RepoRelativePath::new(repo_path.to_owned()).ok_or_else(|| {
        AppError::UnsafeFilesystemEntry {
            path: ctx.repo_root.join(repo_path),
            reason: "relink repository path is not normalized",
        }
    })?;
    let absolute = ctx.repo_store.join(item_store_path);
    let relative =
        absolute
            .strip_prefix(&ctx.config.store)
            .map_err(|_| AppError::UnsafeFilesystemEntry {
                path: absolute.clone(),
                reason: "relink store path escapes the configured store root",
            })?;
    let store_path = StoreRelativePath::new(relative.to_string_lossy().replace('\\', "/")).ok_or(
        AppError::UnsafeFilesystemEntry {
            path: absolute,
            reason: "relink store path is not normalized",
        },
    )?;
    Ok(MaterializationLocation::new(repo_path, store_path))
}

fn store_relative_path(store_root: &Path, absolute: &Path) -> Result<StoreRelativePath> {
    let relative =
        absolute
            .strip_prefix(store_root)
            .map_err(|_| AppError::UnsafeFilesystemEntry {
                path: absolute.to_path_buf(),
                reason: "relink store path escapes the configured store root",
            })?;
    StoreRelativePath::new(relative.to_string_lossy().replace('\\', "/")).ok_or_else(|| {
        AppError::UnsafeFilesystemEntry {
            path: absolute.to_path_buf(),
            reason: "relink store path is not normalized",
        }
    })
}

fn is_healthy(facts: &crate::fs::materializer::MaterializationFacts) -> bool {
    facts.store_exists
        && facts.store_regular
        && facts.store_hardlink_free
        && facts.hardlink_free
        && (facts.repo_entry_kind == RepoEntryKind::ManagedSymlink
            || (facts.repo_entry_kind == RepoEntryKind::RegularFile
                && facts.copy_content == CopyContentState::Equal))
}
