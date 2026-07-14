//! Explicit, directional synchronization between an attached regular copy and
//! its canonical store file.
//!
//! This is the only Phase 9 operation that may overwrite a diverged Copy.
//! Every write is backed by a full operation record, rather than the
//! artifact-only repair journal, because recovery must distinguish the source
//! of truth from the replaced endpoint.

use std::path::Path;

use ulid::Ulid;

use crate::{
    context::{self, RepoContext},
    domain::{
        materialization::CopyContentState,
        operation_record::{
            OperationDirection, OperationKind, OperationPhase, OperationPreState, OperationRecord,
            RecoveryAbsolutePath, RecoveryRecord, RecoveryRecordKind,
            OPERATION_RECORD_SCHEMA_VERSION,
        },
        ownership::OwnershipState,
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
        mutation_journal::SyncMutationJournal,
    },
    git,
    ignore::IgnoreBackend,
    plan::item_sync::{
        ItemSyncAction, ItemSyncPlan, ItemSyncReport, ItemSyncRequest, SyncDirection, SyncOutcome,
    },
    policy::sync_policy::{self, SyncDecision, SyncMaterializationState},
    storage::operation_record_store,
};

use super::path::repo_relative_string;

pub(crate) fn sync_report(
    ctx: &mut RepoContext,
    abs_path: &Path,
    request: ItemSyncRequest,
    ignore: &dyn IgnoreBackend,
) -> Result<ItemSyncReport> {
    let plan = build_sync_plan(ctx, abs_path, request.direction, ignore)?;
    if request.dry_run {
        return Ok(ItemSyncReport {
            outcome: dry_run_outcome(plan.action),
            plan,
            dry_run: true,
        });
    }

    if plan.action == ItemSyncAction::ReplaceStoreFromRepo && !request.confirmed {
        return Err(AppError::SyncConfirmationRequired);
    }

    let outcome = execute_sync_plan(ctx, &plan, ignore)?;
    Ok(ItemSyncReport {
        plan,
        outcome,
        dry_run: false,
    })
}

fn build_sync_plan(
    ctx: &RepoContext,
    abs_path: &Path,
    direction: SyncDirection,
    ignore: &dyn IgnoreBackend,
) -> Result<ItemSyncPlan> {
    let path = repo_relative_string(&ctx.repo_root, abs_path)?;
    let item = ctx
        .manifest
        .get(&path)
        .ok_or_else(|| AppError::NotManagedLink {
            path: abs_path.to_path_buf(),
        })?;
    if item.ownership_state != OwnershipState::Attached {
        return Err(AppError::SyncRequiresRegularCopy {
            path: abs_path.to_path_buf(),
        });
    }
    if git::is_tracked(&ctx.repo_root, abs_path)? {
        return Err(AppError::PathIsTracked {
            path: abs_path.to_path_buf(),
        });
    }
    if !ignore.has_entry(&ctx.repo_root, &path)? {
        return Err(AppError::Internal(
            "managed sync exclude is missing; synchronize only an attached ignored item".into(),
        ));
    }

    let location = materialization_location(ctx, &path, &item.store_path)?;
    let store_path = ctx.config.store.join(location.store_path.as_str());
    let materializer = DefaultMaterializer::new(ctx.repo_root.clone(), ctx.config.store.clone());
    let facts = materializer.inspect(MaterializationInspectionRequest {
        location,
        purpose: InspectionPurpose::Planning,
    })?;
    validate_store_facts(&store_path, &facts)?;
    if facts.repo_entry_kind == RepoEntryKind::RegularFile && !facts.hardlink_free {
        return Err(AppError::HardlinkedFile {
            path: abs_path.to_path_buf(),
        });
    }

    let state = sync_state(&facts);
    let action = match sync_policy::decide_sync(direction, state) {
        SyncDecision::Action(action) => action,
        SyncDecision::MissingNeedsRepair => {
            return Err(AppError::SyncMaterializationMissing {
                path: abs_path.to_path_buf(),
            });
        }
        SyncDecision::RequiresAttachedRegularCopy => {
            return Err(AppError::SyncRequiresRegularCopy {
                path: abs_path.to_path_buf(),
            });
        }
        SyncDecision::InspectionFailed => {
            return Err(AppError::UnsafeFilesystemEntry {
                path: abs_path.to_path_buf(),
                reason: "sync requires an isolated inspectable regular copy",
            });
        }
    };

    Ok(ItemSyncPlan {
        path,
        abs_path: abs_path.to_path_buf(),
        store_path,
        direction,
        action,
    })
}

fn sync_state(facts: &crate::fs::materializer::MaterializationFacts) -> SyncMaterializationState {
    match facts.repo_entry_kind {
        RepoEntryKind::ManagedSymlink => SyncMaterializationState::ManagedSymlink,
        RepoEntryKind::RegularFile if facts.hardlink_free => {
            SyncMaterializationState::RegularCopy(facts.copy_content)
        }
        RepoEntryKind::Missing => SyncMaterializationState::Missing,
        RepoEntryKind::RegularFile
        | RepoEntryKind::UnmanagedSymlinkOrReparsePoint
        | RepoEntryKind::Directory
        | RepoEntryKind::Other => SyncMaterializationState::Unsafe,
    }
}

fn validate_store_facts(
    store_path: &Path,
    facts: &crate::fs::materializer::MaterializationFacts,
) -> Result<()> {
    if !facts.store_exists {
        return Err(AppError::StoreMissing {
            path: store_path.to_path_buf(),
            store_path: store_path.to_path_buf(),
        });
    }
    if !facts.store_regular || !facts.store_hardlink_free {
        return Err(AppError::UnsafeFilesystemEntry {
            path: store_path.to_path_buf(),
            reason: "sync store entry is not an isolated regular file",
        });
    }
    Ok(())
}

fn materialization_location(
    ctx: &RepoContext,
    repo_path: &str,
    item_store_path: &str,
) -> Result<MaterializationLocation> {
    let repo_path = RepoRelativePath::new(repo_path.to_owned()).ok_or_else(|| {
        AppError::UnsafeFilesystemEntry {
            path: ctx.repo_root.join(repo_path),
            reason: "sync repository path is not normalized",
        }
    })?;
    let store_absolute = ctx.repo_store.join(item_store_path);
    let store_relative = store_absolute
        .strip_prefix(&ctx.config.store)
        .map_err(|_| AppError::UnsafeFilesystemEntry {
            path: store_absolute.clone(),
            reason: "sync store path escapes the configured store root",
        })?;
    let store_path = StoreRelativePath::new(store_relative.to_string_lossy().replace('\\', "/"))
        .ok_or(AppError::UnsafeFilesystemEntry {
            path: store_absolute,
            reason: "sync store path is not normalized",
        })?;
    Ok(MaterializationLocation::new(repo_path, store_path))
}

fn dry_run_outcome(action: ItemSyncAction) -> SyncOutcome {
    match action {
        ItemSyncAction::AlreadySynchronized => SyncOutcome::AlreadySynchronized,
        ItemSyncAction::ManagedSymlinkNoOp => SyncOutcome::ManagedSymlinkNoOp,
        ItemSyncAction::ReplaceRepoFromStore => SyncOutcome::WouldSynchronizeFromStore,
        ItemSyncAction::ReplaceStoreFromRepo => SyncOutcome::WouldSynchronizeFromRepo,
    }
}

fn execute_sync_plan(
    ctx: &mut RepoContext,
    plan: &ItemSyncPlan,
    ignore: &dyn IgnoreBackend,
) -> Result<SyncOutcome> {
    match plan.action {
        ItemSyncAction::AlreadySynchronized => return Ok(SyncOutcome::AlreadySynchronized),
        ItemSyncAction::ManagedSymlinkNoOp => return Ok(SyncOutcome::ManagedSymlinkNoOp),
        ItemSyncAction::ReplaceRepoFromStore | ItemSyncAction::ReplaceStoreFromRepo => {}
    }

    let location = materialization_location(
        ctx,
        &plan.path,
        &ctx.manifest
            .get(&plan.path)
            .ok_or_else(|| {
                AppError::Internal("sync plan item disappeared from the manifest".into())
            })?
            .store_path,
    )?;
    let mut materializer =
        DefaultMaterializer::new(ctx.repo_root.clone(), ctx.config.store.clone());
    let planning_facts = materializer.inspect(MaterializationInspectionRequest {
        location: location.clone(),
        purpose: InspectionPurpose::PreCommit,
    })?;
    validate_write_facts(ctx, plan, ignore, &planning_facts, false)?;

    let repo_fingerprint = RecoveryFingerprint::from_file(&plan.abs_path)?;
    let store_fingerprint = RecoveryFingerprint::from_file(&plan.store_path)?;
    if repo_fingerprint == store_fingerprint {
        return Err(AppError::FilesystemEntryChanged {
            path: plan.abs_path.clone(),
        });
    }
    let mut record = sync_record(
        ctx,
        plan,
        location.store_path.clone(),
        repo_fingerprint.clone(),
        store_fingerprint.clone(),
    )?;
    operation_record_store::create(&ctx.config.store, &record)?;

    let mut journal = SyncMutationJournal::new(
        &ctx.config.store,
        &ctx.repo_root,
        ignore,
        &mut record,
        plan.abs_path.clone(),
        plan.store_path.clone(),
    );

    match plan.action {
        ItemSyncAction::ReplaceRepoFromStore => {
            let action = MaterializationAction::Replace {
                location: location.clone(),
                strategy: crate::domain::materialization::MaterializationStrategy::Copy,
                expected: planning_facts.expected(),
            };
            let prepared = materializer.prepare(action, &mut journal)?;
            let facts = materializer.inspect(MaterializationInspectionRequest {
                location: location.clone(),
                purpose: InspectionPurpose::PreCommit,
            })?;
            validate_write_facts(ctx, plan, ignore, &facts, true)?;
            validate_recorded_fingerprints(plan, &repo_fingerprint, &store_fingerprint)?;
            let permit = journal
                .issue_commit_permit(facts.write_precondition_guard(prepared.commit_context()))?;
            materializer.commit(prepared, permit)?;
        }
        ItemSyncAction::ReplaceStoreFromRepo => {
            let action = CanonicalTransferAction::CopyFromRepo {
                source: location.repo_path.clone(),
                destination: location.store_path.clone(),
                expected_source: canonical_expected(
                    ctx,
                    &location,
                    CanonicalInspectionPurpose::Planning,
                    true,
                )?,
                expected_destination: canonical_expected(
                    ctx,
                    &location,
                    CanonicalInspectionPurpose::Planning,
                    false,
                )?,
            };
            let mut transfer =
                DefaultCanonicalTransfer::new(ctx.repo_root.clone(), ctx.config.store.clone());
            let prepared = transfer.prepare(action.clone(), &mut journal)?;
            let facts = transfer.inspect(CanonicalTransferInspectionRequest {
                action,
                purpose: CanonicalInspectionPurpose::PreCommit,
            })?;
            validate_transfer_write_facts(ctx, plan, ignore, &facts)?;
            validate_recorded_fingerprints(plan, &repo_fingerprint, &store_fingerprint)?;
            let permit = journal
                .issue_commit_permit(facts.write_precondition_guard(prepared.commit_context()))?;
            transfer.commit(prepared, permit)?;
        }
        ItemSyncAction::AlreadySynchronized | ItemSyncAction::ManagedSymlinkNoOp => unreachable!(),
    }

    journal.advance_content_synchronized()?;
    let post = materializer.inspect(MaterializationInspectionRequest {
        location,
        purpose: InspectionPurpose::PostCommit,
    })?;
    validate_postconditions(ctx, plan, ignore, &post)?;
    validate_synchronized_fingerprints(plan, &repo_fingerprint, &store_fingerprint)?;
    journal.advance_post_commit_validated()?;
    journal.cleanup_all()?;
    drop(journal);
    operation_record_store::remove(&ctx.config.store, &record.record_id)?;

    Ok(match plan.action {
        ItemSyncAction::ReplaceRepoFromStore => SyncOutcome::SynchronizedFromStore,
        ItemSyncAction::ReplaceStoreFromRepo => SyncOutcome::SynchronizedFromRepo,
        ItemSyncAction::AlreadySynchronized | ItemSyncAction::ManagedSymlinkNoOp => unreachable!(),
    })
}

/// A file can be modified in place without changing its identity. The opaque
/// adapter snapshots protect replacement and hardlink races; this operation
/// layer additionally checks both durable content observations immediately
/// before the permit so sync never silently applies a decision made from an
/// older byte stream.
fn validate_recorded_fingerprints(
    plan: &ItemSyncPlan,
    expected_repo: &RecoveryFingerprint,
    expected_store: &RecoveryFingerprint,
) -> Result<()> {
    if RecoveryFingerprint::from_file(&plan.abs_path)? != *expected_repo {
        return Err(AppError::FilesystemEntryChanged {
            path: plan.abs_path.clone(),
        });
    }
    if RecoveryFingerprint::from_file(&plan.store_path)? != *expected_store {
        return Err(AppError::FilesystemEntryChanged {
            path: plan.store_path.clone(),
        });
    }
    Ok(())
}

fn validate_synchronized_fingerprints(
    plan: &ItemSyncPlan,
    repo_before: &RecoveryFingerprint,
    store_before: &RecoveryFingerprint,
) -> Result<()> {
    let expected = match plan.direction {
        SyncDirection::FromStore => store_before,
        SyncDirection::FromRepo => repo_before,
    };
    if RecoveryFingerprint::from_file(&plan.abs_path)? != *expected
        || RecoveryFingerprint::from_file(&plan.store_path)? != *expected
    {
        return Err(AppError::Internal(
            "sync content postconditions failed; durable recovery record was retained".into(),
        ));
    }
    Ok(())
}

fn sync_record(
    ctx: &RepoContext,
    plan: &ItemSyncPlan,
    store_path: StoreRelativePath,
    repo_fingerprint: RecoveryFingerprint,
    store_fingerprint: RecoveryFingerprint,
) -> Result<RecoveryRecord> {
    let direction = match plan.direction {
        SyncDirection::FromStore => OperationDirection::FromStore,
        SyncDirection::FromRepo => OperationDirection::FromRepo,
    };
    Ok(RecoveryRecord {
        schema_version: OPERATION_RECORD_SCHEMA_VERSION,
        record_id: Ulid::new().to_string(),
        created_at: context::now_iso8601(),
        record: RecoveryRecordKind::Operation(OperationRecord {
            operation: OperationKind::Sync,
            phase: OperationPhase::RecordCreated,
            repo_id: ctx.repo_id.clone(),
            repo_root: RecoveryAbsolutePath::new(&ctx.repo_root).ok_or_else(|| {
                AppError::UnsafeFilesystemEntry {
                    path: ctx.repo_root.clone(),
                    reason: "repository root is not a safe absolute path",
                }
            })?,
            repo_store_path: None,
            strategy: crate::domain::materialization::MaterializationStrategy::Copy,
            direction: Some(direction),
            pre_state: OperationPreState {
                repo_path: Some(RepoRelativePath::new(plan.path.clone()).ok_or_else(|| {
                    AppError::UnsafeFilesystemEntry {
                        path: plan.abs_path.clone(),
                        reason: "sync plan repository path is not normalized",
                    }
                })?),
                store_path: Some(store_path),
                repo_fingerprint: Some(repo_fingerprint),
                store_fingerprint: Some(store_fingerprint),
                manifest_contains_item: Some(true),
                exclude_owned: Some(true),
                final_exclude_owned: Some(true),
            },
            post_state: None,
            artifact_record_ids: Vec::new(),
            backup: None,
        }),
    })
}

fn canonical_expected(
    ctx: &RepoContext,
    location: &MaterializationLocation,
    purpose: CanonicalInspectionPurpose,
    source: bool,
) -> Result<ExpectedCanonicalEntry> {
    let transfer = DefaultCanonicalTransfer::new(ctx.repo_root.clone(), ctx.config.store.clone());
    let action = CanonicalTransferAction::CopyFromRepo {
        source: location.repo_path.clone(),
        destination: location.store_path.clone(),
        expected_source: ExpectedCanonicalEntry::unchecked(CanonicalEntryKind::RegularFile),
        expected_destination: ExpectedCanonicalEntry::unchecked(CanonicalEntryKind::RegularFile),
    };
    let facts = transfer.inspect(CanonicalTransferInspectionRequest { action, purpose })?;
    if source {
        Ok(facts.expected_source())
    } else {
        Ok(facts.expected_destination())
    }
}

fn validate_write_facts(
    ctx: &RepoContext,
    plan: &ItemSyncPlan,
    ignore: &dyn IgnoreBackend,
    facts: &crate::fs::materializer::MaterializationFacts,
    immediately_before_permit: bool,
) -> Result<()> {
    validate_store_facts(&plan.store_path, facts)?;
    if facts.repo_entry_kind != RepoEntryKind::RegularFile || !facts.hardlink_free {
        return Err(AppError::SyncRequiresRegularCopy {
            path: plan.abs_path.clone(),
        });
    }
    if facts.copy_content != CopyContentState::Diverged {
        return Err(AppError::FilesystemEntryChanged {
            path: plan.abs_path.clone(),
        });
    }
    validate_repo_integration(ctx, plan, ignore)?;
    if immediately_before_permit {
        // Tests may mutate Git or excludes at exactly the operation/journal
        // boundary. The journal repeats those checks before issuing its permit.
        failpoint::after(Failpoint::WritePreconditionsValidated)?;
    }
    Ok(())
}

fn validate_transfer_write_facts(
    ctx: &RepoContext,
    plan: &ItemSyncPlan,
    ignore: &dyn IgnoreBackend,
    facts: &crate::fs::canonical_transfer::CanonicalTransferFacts,
) -> Result<()> {
    if facts.source_kind != CanonicalEntryKind::RegularFile
        || facts.destination_kind != CanonicalEntryKind::RegularFile
        || !facts.source_hardlink_free
        || !facts.destination_hardlink_free
    {
        return Err(AppError::UnsafeFilesystemEntry {
            path: plan.abs_path.clone(),
            reason: "sync requires isolated regular repository and store entries",
        });
    }
    validate_repo_integration(ctx, plan, ignore)?;
    failpoint::after(Failpoint::WritePreconditionsValidated)
}

fn validate_repo_integration(
    ctx: &RepoContext,
    plan: &ItemSyncPlan,
    ignore: &dyn IgnoreBackend,
) -> Result<()> {
    if git::is_tracked(&ctx.repo_root, &plan.abs_path)? {
        return Err(AppError::PathIsTracked {
            path: plan.abs_path.clone(),
        });
    }
    if !ignore.has_entry(&ctx.repo_root, &plan.path)? {
        return Err(AppError::Internal(
            "managed sync exclude was removed before commit".into(),
        ));
    }
    Ok(())
}

fn validate_postconditions(
    ctx: &RepoContext,
    plan: &ItemSyncPlan,
    ignore: &dyn IgnoreBackend,
    facts: &crate::fs::materializer::MaterializationFacts,
) -> Result<()> {
    validate_store_facts(&plan.store_path, facts)?;
    if facts.repo_entry_kind != RepoEntryKind::RegularFile
        || !facts.hardlink_free
        || facts.copy_content != CopyContentState::Equal
    {
        return Err(AppError::Internal(
            "sync postconditions failed; durable recovery record was retained".into(),
        ));
    }
    validate_repo_integration(ctx, plan, ignore)
}

/// Applies the Phase 9 recovery table for a sync record owned by the current
/// checkout. It intentionally never guesses how to repair an unexpected
/// endpoint: the only safe automatic states are the recorded pre-state and
/// the direction-specific synchronized state.
pub(crate) fn recover_if_owned(
    store_root: &Path,
    current_repo_root: &Path,
    record: &mut RecoveryRecord,
) -> Result<bool> {
    use crate::policy::recovery_policy::{
        self, RecoveryDecision, RecoveryFacts, RecoveryForward, RecoveryRollback,
    };

    let owned = matches!(
        &record.record,
        RecoveryRecordKind::Operation(operation)
            if operation.operation == OperationKind::Sync
                && !operation.phase.is_finalized()
                && operation.repo_root.as_path() == current_repo_root
    );
    if !owned {
        return Ok(false);
    }

    for _ in 0..3 {
        let operation = operation(record)?.clone();
        let facts = collect_recovery_facts(store_root, current_repo_root, record, &operation)?;
        match recovery_policy::decide(recovery_policy::RecoveryPolicyInput {
            operation: OperationKind::Sync,
            phase: operation.phase,
            direction: operation.direction,
            facts: RecoveryFacts::Sync(facts),
        }) {
            RecoveryDecision::AdvanceRecord { to } => update_phase(store_root, record, to)?,
            RecoveryDecision::Rollback(RecoveryRollback::DeleteRecordOnly) => {
                operation_record_store::remove(store_root, &record.record_id)?;
                return Ok(true);
            }
            RecoveryDecision::Forward(RecoveryForward::VerifyPostconditionsAndComplete) => {
                update_phase(store_root, record, OperationPhase::PostCommitValidated)?;
                operation_record_store::remove(store_root, &record.record_id)?;
                return Ok(true);
            }
            RecoveryDecision::Conflict(conflict) => {
                return Err(blocked(record, conflict.reason()));
            }
            decision => {
                return Err(blocked(
                    record,
                    format!("invalid sync recovery action: {decision:?}"),
                ));
            }
        }
    }

    Err(blocked(
        record,
        "sync recovery exceeded its bounded phase transitions",
    ))
}

fn collect_recovery_facts(
    store_root: &Path,
    repo_root: &Path,
    record: &RecoveryRecord,
    operation: &OperationRecord,
) -> Result<crate::policy::recovery_policy::SyncRecoveryFacts> {
    use crate::policy::recovery_policy::{SyncContentFact, SyncRecoveryFacts};

    let repo_path = operation.pre_state.repo_path.as_ref().ok_or_else(|| {
        blocked(
            record,
            "sync record is missing its repository path observation",
        )
    })?;
    let store_path = operation
        .pre_state
        .store_path
        .as_ref()
        .ok_or_else(|| blocked(record, "sync record is missing its store path observation"))?;
    let repo_before = operation
        .pre_state
        .repo_fingerprint
        .as_ref()
        .ok_or_else(|| blocked(record, "sync record is missing its repository fingerprint"))?;
    let store_before = operation
        .pre_state
        .store_fingerprint
        .as_ref()
        .ok_or_else(|| blocked(record, "sync record is missing its store fingerprint"))?;
    let direction = operation.direction.ok_or_else(|| {
        blocked(
            record,
            "sync record is missing its explicit synchronization direction",
        )
    })?;
    if repo_before == store_before
        || operation.pre_state.manifest_contains_item != Some(true)
        || operation.pre_state.exclude_owned != Some(true)
    {
        return Ok(SyncRecoveryFacts {
            content: SyncContentFact::Unexpected,
        });
    }

    let location = MaterializationLocation::new(repo_path.clone(), store_path.clone());
    let materializer = DefaultMaterializer::new(repo_root.to_path_buf(), store_root.to_path_buf());
    let facts = materializer.inspect(MaterializationInspectionRequest {
        location,
        purpose: InspectionPurpose::PostCommit,
    })?;
    let integrated = !git::is_tracked(repo_root, &repo_root.join(repo_path.as_str()))?
        && crate::ignore::GitInfoExclude.has_entry(repo_root, repo_path.as_str())?;
    if !integrated
        || facts.repo_entry_kind != RepoEntryKind::RegularFile
        || !facts.hardlink_free
        || !facts.store_exists
        || !facts.store_regular
        || !facts.store_hardlink_free
    {
        return Ok(SyncRecoveryFacts {
            content: SyncContentFact::Unexpected,
        });
    }

    let repo_absolute = repo_root.join(repo_path.as_str());
    let store_absolute = store_root.join(store_path.as_str());
    let repo_current = RecoveryFingerprint::from_file(&repo_absolute)?;
    let store_current = RecoveryFingerprint::from_file(&store_absolute)?;
    let content = match direction {
        OperationDirection::FromStore
            if repo_current == *repo_before && store_current == *store_before =>
        {
            SyncContentFact::BeforeReplacement
        }
        OperationDirection::FromStore
            if repo_current == *store_before && store_current == *store_before =>
        {
            SyncContentFact::Synchronized
        }
        OperationDirection::FromRepo
            if repo_current == *repo_before && store_current == *store_before =>
        {
            SyncContentFact::BeforeReplacement
        }
        OperationDirection::FromRepo
            if repo_current == *repo_before && store_current == *repo_before =>
        {
            SyncContentFact::Synchronized
        }
        _ => SyncContentFact::Unexpected,
    };
    Ok(SyncRecoveryFacts { content })
}

fn operation(record: &RecoveryRecord) -> Result<&OperationRecord> {
    match &record.record {
        RecoveryRecordKind::Operation(operation) => Ok(operation),
        RecoveryRecordKind::Artifact(_) => Err(AppError::Internal(
            "expected a sync operation record, received an artifact record".into(),
        )),
    }
}

fn update_phase(
    store_root: &Path,
    record: &mut RecoveryRecord,
    phase: OperationPhase,
) -> Result<()> {
    match &mut record.record {
        RecoveryRecordKind::Operation(operation) => operation.phase = phase,
        RecoveryRecordKind::Artifact(_) => unreachable!(),
    }
    operation_record_store::update(store_root, record)
}

fn blocked(record: &RecoveryRecord, reason: impl Into<String>) -> AppError {
    AppError::RecoveryBlocked {
        record_id: record.record_id.clone(),
        reason: reason.into(),
    }
}
