//! Recovery executors for Phase 10 lifecycle records.
//!
//! The policy module owns admissible rows; this module only collects strict
//! facts and performs the one action selected by a row.  A state that is not
//! exactly current or exactly next stays intact as a `RecoveryBlocked` error.

use std::path::{Path, PathBuf};

use crate::{
    context,
    domain::{
        materialization::MaterializationStrategy,
        operation_record::{
            OperationKind, OperationPhase, OperationRecord, RecoveryRecord, RecoveryRecordKind,
        },
        ownership::OwnershipState,
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
    ignore::{GitInfoExclude, IgnoreBackend},
    policy::recovery_policy::{
        self, DirectionalRelinkContentFact, DirectionalRelinkExcludeFact,
        DirectionalRelinkManifestFact, DirectionalRelinkRecoveryFacts, MoveExcludeFact,
        MoveManifestFact, MoveRecoveryFacts, MoveRepoFact, MoveStoreFact, RecoveryDecision,
        RecoveryFacts, RecoveryForward, RecoveryPolicyInput, RecoveryRollback, RestoreExcludeFact,
        RestoreManifestFact, RestoreRecoveryFacts, RestoreRepoFact, RestoreStoreFact,
    },
    storage::operation_record_store,
    store::manifest,
};

pub(crate) fn recover_if_owned(
    store_root: &Path,
    current_repo_root: &Path,
    record: &mut RecoveryRecord,
) -> Result<bool> {
    let owned = matches!(
        &record.record,
        RecoveryRecordKind::Operation(operation)
            if matches!(operation.operation, OperationKind::Restore | OperationKind::Move | OperationKind::Relink)
                && !operation.phase.is_finalized()
                && operation.repo_root.as_path() == current_repo_root
    );
    if !owned {
        return Ok(false);
    }

    for _ in 0..10 {
        let operation = operation(record)?.clone();
        let facts = match operation.operation {
            OperationKind::Restore => RecoveryFacts::Restore(restore_facts(
                store_root,
                current_repo_root,
                record,
                &operation,
            )?),
            OperationKind::Move => RecoveryFacts::Move(move_facts(
                store_root,
                current_repo_root,
                record,
                &operation,
            )?),
            OperationKind::Relink => RecoveryFacts::DirectionalRelink(relink_facts(
                store_root,
                current_repo_root,
                record,
                &operation,
            )?),
            _ => unreachable!(),
        };
        let decision = recovery_policy::decide(RecoveryPolicyInput {
            operation: operation.operation,
            phase: operation.phase,
            direction: operation.direction,
            facts,
        });
        match decision {
            RecoveryDecision::AdvanceRecord { to } => update_phase(store_root, record, to)?,
            RecoveryDecision::Rollback(RecoveryRollback::DeleteRecordOnly) => {
                operation_record_store::remove(store_root, &record.record_id)?;
                return Ok(true);
            }
            RecoveryDecision::Rollback(RecoveryRollback::RemoveOperationOwnedExclude) => {
                remove_owned_exclude(current_repo_root, &operation)?;
                operation_record_store::remove(store_root, &record.record_id)?;
                return Ok(true);
            }
            RecoveryDecision::Forward(action) => {
                if perform_forward(store_root, current_repo_root, record, &operation, action)? {
                    return Ok(true);
                }
            }
            RecoveryDecision::Conflict(conflict) => return Err(blocked(record, conflict.reason())),
        }
    }
    Err(blocked(
        record,
        "lifecycle recovery exceeded its bounded phase transitions",
    ))
}

fn perform_forward(
    store_root: &Path,
    repo_root: &Path,
    record: &mut RecoveryRecord,
    operation: &OperationRecord,
    action: RecoveryForward,
) -> Result<bool> {
    match (operation.operation, action) {
        (OperationKind::Restore, RecoveryForward::StageStoreIntoBackup) => {
            stage_restore_store(store_root, repo_root, record, operation)?;
        }
        (OperationKind::Restore, RecoveryForward::RemoveManifest) => {
            let (repo_store, path) = repo_store_and_path(store_root, record, operation)?;
            let mut manifest = manifest::load(&repo_store)?;
            manifest.remove(path);
            manifest::save(&repo_store, &manifest)?;
            update_phase(store_root, record, OperationPhase::ManifestRemoved)?;
        }
        (OperationKind::Restore, RecoveryForward::ApplyKeepIgnorePolicy) => {
            let (_, path) = repo_store_and_path(store_root, record, operation)?;
            if operation.pre_state.final_exclude_owned == Some(false) {
                GitInfoExclude.remove_entries(repo_root, &[path])?;
            }
            update_phase(store_root, record, OperationPhase::ExcludeUpdated)?;
        }
        (OperationKind::Restore, RecoveryForward::DeleteBackupAndComplete) => {
            let backup = operation
                .backup
                .as_ref()
                .ok_or_else(|| blocked(record, "restore record has no recovery backup"))?;
            operation_record_store::cleanup_backup(
                store_root,
                repo_root,
                &record.record_id,
                backup,
            )?;
            update_phase(store_root, record, OperationPhase::PostCommitValidated)?;
            operation_record_store::remove(store_root, &record.record_id)?;
            return Ok(true);
        }
        (OperationKind::Move, RecoveryForward::MoveRepoMaterialization) => {
            recover_move_repo(store_root, repo_root, record, operation)?;
        }
        (OperationKind::Move, RecoveryForward::SaveManifest) => {
            let (repo_store, old) = repo_store_and_path(store_root, record, operation)?;
            let post = post_paths(store_root, record, operation)?;
            let mut manifest = manifest::load(&repo_store)?;
            let relative_store = store_root
                .join(post.store_path.as_str())
                .strip_prefix(&repo_store)
                .map_err(|_| {
                    blocked(
                        record,
                        "move destination store path is outside its repo store",
                    )
                })?
                .to_string_lossy()
                .replace('\\', "/");
            manifest.rename(
                old,
                post.repo_path.as_str(),
                &relative_store,
                &context::now_iso8601(),
            );
            manifest::save(&repo_store, &manifest)?;
            update_phase(store_root, record, OperationPhase::ManifestSaved)?;
        }
        (OperationKind::Move, RecoveryForward::RemoveOldExclude) => {
            let (_, old) = repo_store_and_path(store_root, record, operation)?;
            GitInfoExclude.remove_entries(repo_root, &[old])?;
            update_phase(store_root, record, OperationPhase::ExcludeFinalized)?;
        }
        (OperationKind::Move, RecoveryForward::VerifyPostconditionsAndComplete) => {
            update_phase(store_root, record, OperationPhase::PostCommitValidated)?;
            operation_record_store::remove(store_root, &record.record_id)?;
            return Ok(true);
        }
        (OperationKind::Relink, RecoveryForward::AttachOwnership) => {
            let (repo_store, path) = repo_store_and_path(store_root, record, operation)?;
            let mut manifest = manifest::load(&repo_store)?;
            manifest.set_ownership_state(path, OwnershipState::Attached, &context::now_iso8601());
            manifest::save(&repo_store, &manifest)?;
            update_phase(store_root, record, OperationPhase::OwnershipAttached)?;
        }
        (OperationKind::Relink, RecoveryForward::DeleteBackupAndComplete) => {
            let backup = operation
                .backup
                .as_ref()
                .ok_or_else(|| blocked(record, "directional relink record has no backup"))?;
            operation_record_store::cleanup_backup(
                store_root,
                repo_root,
                &record.record_id,
                backup,
            )?;
            update_phase(store_root, record, OperationPhase::PostCommitValidated)?;
            operation_record_store::remove(store_root, &record.record_id)?;
            return Ok(true);
        }
        _ => {
            return Err(blocked(
                record,
                format!("invalid lifecycle recovery action: {action:?}"),
            ))
        }
    }
    Ok(false)
}

fn stage_restore_store(
    store_root: &Path,
    repo_root: &Path,
    record: &mut RecoveryRecord,
    operation: &OperationRecord,
) -> Result<()> {
    let repo_path = operation
        .pre_state
        .repo_path
        .clone()
        .ok_or_else(|| blocked(record, "restore path missing"))?;
    let source = operation
        .pre_state
        .store_path
        .clone()
        .ok_or_else(|| blocked(record, "restore store path missing"))?;
    let backup = operation
        .backup
        .as_ref()
        .ok_or_else(|| blocked(record, "restore backup missing"))?;
    let crate::domain::operation_record::ArtifactLocation::Store { path: destination } =
        &backup.location
    else {
        return Err(blocked(record, "restore backup is not store-side"));
    };
    let repo = repo_root.join(repo_path.as_str());
    let backup_abs = store_root.join(destination.as_str());
    let mut journal = AddMutationJournal::new(
        store_root,
        repo_root,
        &GitInfoExclude,
        record,
        repo,
        backup_abs.clone(),
    );
    journal.ensure_store_destination_parent()?;
    let mut transfer =
        DefaultCanonicalTransfer::new(repo_root.to_path_buf(), store_root.to_path_buf());
    let inspection = CanonicalTransferAction::Move {
        source: source.clone(),
        destination: destination.clone(),
        expected_source: ExpectedCanonicalEntry::unchecked(CanonicalEntryKind::RegularFile),
        expected_destination: ExpectedCanonicalEntry::unchecked(CanonicalEntryKind::Missing),
    };
    let facts = transfer.inspect(CanonicalTransferInspectionRequest {
        action: inspection,
        purpose: CanonicalInspectionPurpose::Planning,
    })?;
    if facts.source_kind != CanonicalEntryKind::RegularFile
        || facts.destination_kind != CanonicalEntryKind::Missing
    {
        return Err(blocked(record, "restore staging inputs changed"));
    }
    let action = CanonicalTransferAction::Move {
        source,
        destination: destination.clone(),
        expected_source: facts.expected_source(),
        expected_destination: facts.expected_destination(),
    };
    let prepared = transfer.prepare(action.clone(), &mut journal)?;
    let fresh = transfer.inspect(CanonicalTransferInspectionRequest {
        action,
        purpose: CanonicalInspectionPurpose::PreCommit,
    })?;
    let permit =
        journal.issue_commit_permit(fresh.write_precondition_guard(prepared.commit_context()))?;
    transfer.commit(prepared, permit)?;
    journal.record_backup_from_path(&backup_abs)?;
    journal.cleanup_all()?;
    journal.advance(OperationPhase::StoreStaged)
}

fn recover_move_repo(
    store_root: &Path,
    repo_root: &Path,
    record: &mut RecoveryRecord,
    operation: &OperationRecord,
) -> Result<()> {
    let old_repo = operation
        .pre_state
        .repo_path
        .clone()
        .ok_or_else(|| blocked(record, "move old repo path missing"))?;
    let old_store = operation
        .pre_state
        .store_path
        .clone()
        .ok_or_else(|| blocked(record, "move old store path missing"))?;
    let post = post_paths(store_root, record, operation)?;
    let old_location = MaterializationLocation::new(old_repo.clone(), old_store);
    let new_location =
        MaterializationLocation::new(post.repo_path.clone(), post.store_path.clone());
    let mut materializer =
        DefaultMaterializer::new(repo_root.to_path_buf(), store_root.to_path_buf());
    let old = materializer.inspect(MaterializationInspectionRequest {
        location: old_location.clone(),
        purpose: InspectionPurpose::PreCommit,
    })?;
    if !is_materialized(&old, operation.strategy) {
        return Err(blocked(record, "move old materialization changed"));
    }
    let mut journal = AddMutationJournal::new(
        store_root,
        repo_root,
        &GitInfoExclude,
        record,
        repo_root.join(post.repo_path.as_str()),
        store_root.join(post.store_path.as_str()),
    );
    let remove = MaterializationAction::Remove {
        location: old_location,
        expected: old.expected(),
    };
    let prepared = materializer.prepare(remove, &mut journal)?;
    let permit =
        journal.issue_commit_permit(old.write_precondition_guard(prepared.commit_context()))?;
    materializer.commit(prepared, permit)?;
    let new = materializer.inspect(MaterializationInspectionRequest {
        location: new_location.clone(),
        purpose: InspectionPurpose::PreCommit,
    })?;
    if new.repo_entry_kind != RepoEntryKind::Missing {
        return Err(blocked(record, "move new materialization appeared"));
    }
    let mut applied_strategy = operation.strategy;
    match create_move_materialization(
        &mut materializer,
        &mut journal,
        &post,
        new_location.clone(),
        operation.strategy,
    ) {
        Ok(()) => {}
        Err(error)
            if operation.strategy == MaterializationStrategy::Symlink
                && is_windows_symlink_unavailable(&error) =>
        {
            create_move_materialization(
                &mut materializer,
                &mut journal,
                &post,
                new_location,
                MaterializationStrategy::Copy,
            )?;
            applied_strategy = MaterializationStrategy::Copy;
        }
        Err(error) => return Err(error),
    }
    journal.cleanup_all()?;
    journal.advance(OperationPhase::RepoMoved)?;
    drop(journal);
    if applied_strategy != operation.strategy {
        if let RecoveryRecordKind::Operation(operation) = &mut record.record {
            operation.strategy = applied_strategy;
        }
        operation_record_store::update(store_root, record)?;
    }
    Ok(())
}

fn create_move_materialization(
    materializer: &mut DefaultMaterializer,
    journal: &mut AddMutationJournal<'_>,
    post: &PostPaths,
    location: MaterializationLocation,
    strategy: MaterializationStrategy,
) -> Result<()> {
    let prepared = materializer.prepare(
        MaterializationAction::Create { location, strategy },
        journal,
    )?;
    let fresh = materializer.inspect(MaterializationInspectionRequest {
        location: MaterializationLocation::new(post.repo_path.clone(), post.store_path.clone()),
        purpose: InspectionPurpose::PreCommit,
    })?;
    let permit =
        journal.issue_commit_permit(fresh.write_precondition_guard(prepared.commit_context()))?;
    materializer.commit(prepared, permit)?;
    Ok(())
}

fn is_windows_symlink_unavailable(error: &AppError) -> bool {
    #[cfg(windows)]
    {
        matches!(error, AppError::Internal(message) if message.contains("Windows symlink creation is unavailable."))
    }
    #[cfg(not(windows))]
    {
        let _ = error;
        false
    }
}

fn restore_facts(
    store_root: &Path,
    repo_root: &Path,
    record: &RecoveryRecord,
    operation: &OperationRecord,
) -> Result<RestoreRecoveryFacts> {
    let (repo_store, path) = repo_store_and_path(store_root, record, operation)?;
    let location = location(operation, record)?;
    let materializer = DefaultMaterializer::new(repo_root.to_path_buf(), store_root.to_path_buf());
    let materialization = materializer.inspect(MaterializationInspectionRequest {
        location,
        purpose: InspectionPurpose::PostCommit,
    })?;
    let canonical = store_root.join(operation.pre_state.store_path.as_ref().unwrap().as_str());
    let backup = backup_path(store_root, record, operation)?;
    let expected = operation.pre_state.store_fingerprint.as_ref();
    let repo = if is_original_restore(&materialization, operation.strategy) {
        RestoreRepoFact::OriginalMaterialization
    } else if materialization.repo_entry_kind == RepoEntryKind::RegularFile
        && materialization.hardlink_free
        && expected.is_some_and(|f| fingerprint_is(&repo_root.join(path), f))
    {
        RestoreRepoFact::RegularMatchingCanonical
    } else {
        RestoreRepoFact::Unexpected
    };
    let store = if expected.is_some_and(|f| fingerprint_is(&canonical, f)) && !backup.exists() {
        RestoreStoreFact::CanonicalPresent
    } else if !canonical.exists() && expected.is_some_and(|f| fingerprint_is(&backup, f)) {
        RestoreStoreFact::CanonicalMissingBackupPresent
    } else {
        RestoreStoreFact::Unexpected
    };
    let manifest = match manifest::load(&repo_store)?.contains(path) {
        true => RestoreManifestFact::Present,
        false => RestoreManifestFact::Absent,
    };
    let excluded = GitInfoExclude.has_entry(repo_root, path)?;
    let exclude = classify_restore_exclude(operation, excluded);
    Ok(RestoreRecoveryFacts {
        repo,
        store,
        manifest,
        exclude,
    })
}

fn move_facts(
    store_root: &Path,
    repo_root: &Path,
    record: &RecoveryRecord,
    operation: &OperationRecord,
) -> Result<MoveRecoveryFacts> {
    let (repo_store, old_path) = repo_store_and_path(store_root, record, operation)?;
    let post = post_paths(store_root, record, operation)?;
    let old_location = location(operation, record)?;
    let new_location =
        MaterializationLocation::new(post.repo_path.clone(), post.store_path.clone());
    let materializer = DefaultMaterializer::new(repo_root.to_path_buf(), store_root.to_path_buf());
    let old = materializer.inspect(MaterializationInspectionRequest {
        location: old_location,
        purpose: InspectionPurpose::PostCommit,
    })?;
    let new = materializer.inspect(MaterializationInspectionRequest {
        location: new_location,
        purpose: InspectionPurpose::PostCommit,
    })?;
    let repo = if is_materialized(&old, operation.strategy)
        && new.repo_entry_kind == RepoEntryKind::Missing
    {
        MoveRepoFact::OldMaterializedNewMissing
    } else if old.repo_entry_kind == RepoEntryKind::Missing
        && is_materialized(&new, operation.strategy)
    {
        MoveRepoFact::OldMissingNewMaterialized
    } else {
        MoveRepoFact::Unexpected
    };
    let fingerprint = operation.pre_state.store_fingerprint.as_ref();
    let old_store = store_root.join(operation.pre_state.store_path.as_ref().unwrap().as_str());
    let new_store = store_root.join(post.store_path.as_str());
    let store = if fingerprint.is_some_and(|f| fingerprint_is(&old_store, f)) && !new_store.exists()
    {
        MoveStoreFact::OldPresentNewMissing
    } else if !old_store.exists() && fingerprint.is_some_and(|f| fingerprint_is(&new_store, f)) {
        MoveStoreFact::OldMissingNewPresent
    } else {
        MoveStoreFact::Unexpected
    };
    let manifest_loaded = manifest::load(&repo_store)?;
    let manifest = match (
        manifest_loaded.contains(old_path),
        manifest_loaded.contains(post.repo_path.as_str()),
    ) {
        (true, false) => MoveManifestFact::OldPath,
        (false, true) => MoveManifestFact::NewPath,
        _ => MoveManifestFact::Unexpected,
    };
    let old_exclude = GitInfoExclude.has_entry(repo_root, old_path)?;
    let new_exclude = GitInfoExclude.has_entry(repo_root, post.repo_path.as_str())?;
    let exclude = match (old_exclude, new_exclude) {
        (true, false) => MoveExcludeFact::BeforeState,
        (true, true) => MoveExcludeFact::OldAndNew,
        (false, true) => MoveExcludeFact::NewOnly,
        _ => MoveExcludeFact::Unexpected,
    };
    Ok(MoveRecoveryFacts {
        repo,
        store,
        manifest,
        exclude,
    })
}

fn relink_facts(
    store_root: &Path,
    repo_root: &Path,
    record: &RecoveryRecord,
    operation: &OperationRecord,
) -> Result<DirectionalRelinkRecoveryFacts> {
    let (repo_store, path) = repo_store_and_path(store_root, record, operation)?;
    let repo = repo_root.join(path);
    let store = store_root.join(
        operation
            .pre_state
            .store_path
            .as_ref()
            .ok_or_else(|| blocked(record, "relink store path missing"))?
            .as_str(),
    );
    let backup = backup_path(store_root, record, operation)?;
    let repo_before = operation
        .pre_state
        .repo_fingerprint
        .as_ref()
        .ok_or_else(|| blocked(record, "relink repo fingerprint missing"))?;
    let store_before = operation
        .pre_state
        .store_fingerprint
        .as_ref()
        .ok_or_else(|| blocked(record, "relink store fingerprint missing"))?;
    let content = if fingerprint_is(&repo, repo_before)
        && fingerprint_is(&store, store_before)
        && !backup.exists()
    {
        DirectionalRelinkContentFact::BeforeDirectionBackupAbsent
    } else {
        let expected = match operation.direction {
            Some(crate::domain::operation_record::OperationDirection::FromStore) => store_before,
            Some(crate::domain::operation_record::OperationDirection::FromRepo) => repo_before,
            None => return Ok(unexpected_relink_facts()),
        };
        let losing = match operation.direction {
            Some(crate::domain::operation_record::OperationDirection::FromStore) => repo_before,
            Some(crate::domain::operation_record::OperationDirection::FromRepo) => store_before,
            None => unreachable!(),
        };
        if fingerprint_is(&repo, expected)
            && fingerprint_is(&store, expected)
            && fingerprint_is(&backup, losing)
        {
            DirectionalRelinkContentFact::SynchronizedBackupPresent
        } else {
            DirectionalRelinkContentFact::Unexpected
        }
    };
    let manifest = match manifest::load(&repo_store)?
        .get(path)
        .map(|item| item.ownership_state)
    {
        Some(OwnershipState::Detached) => DirectionalRelinkManifestFact::Detached,
        Some(OwnershipState::Attached) => DirectionalRelinkManifestFact::Attached,
        _ => DirectionalRelinkManifestFact::Unexpected,
    };
    let excluded = GitInfoExclude.has_entry(repo_root, path)?;
    let exclude = if excluded {
        DirectionalRelinkExcludeFact::Present
    } else {
        DirectionalRelinkExcludeFact::BeforeState
    };
    Ok(DirectionalRelinkRecoveryFacts {
        content,
        manifest,
        exclude,
    })
}

fn unexpected_relink_facts() -> DirectionalRelinkRecoveryFacts {
    DirectionalRelinkRecoveryFacts {
        content: DirectionalRelinkContentFact::Unexpected,
        manifest: DirectionalRelinkManifestFact::Unexpected,
        exclude: DirectionalRelinkExcludeFact::Unexpected,
    }
}

fn operation(record: &RecoveryRecord) -> Result<&OperationRecord> {
    match &record.record {
        RecoveryRecordKind::Operation(operation) => Ok(operation),
        RecoveryRecordKind::Artifact(_) => {
            Err(AppError::Internal("expected operation record".into()))
        }
    }
}
fn update_phase(
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
fn blocked(record: &RecoveryRecord, reason: impl Into<String>) -> AppError {
    AppError::RecoveryBlocked {
        record_id: record.record_id.clone(),
        reason: reason.into(),
    }
}

fn repo_store_and_path<'a>(
    store_root: &Path,
    record: &RecoveryRecord,
    operation: &'a OperationRecord,
) -> Result<(PathBuf, &'a str)> {
    let root = operation
        .repo_store_path
        .as_ref()
        .ok_or_else(|| blocked(record, "lifecycle record lacks repository store path"))?;
    let path = operation
        .pre_state
        .repo_path
        .as_ref()
        .ok_or_else(|| blocked(record, "lifecycle record lacks repository path"))?;
    Ok((store_root.join(root.as_str()), path.as_str()))
}
fn location(
    operation: &OperationRecord,
    record: &RecoveryRecord,
) -> Result<MaterializationLocation> {
    Ok(MaterializationLocation::new(
        operation
            .pre_state
            .repo_path
            .clone()
            .ok_or_else(|| blocked(record, "repository path missing"))?,
        operation
            .pre_state
            .store_path
            .clone()
            .ok_or_else(|| blocked(record, "store path missing"))?,
    ))
}
struct PostPaths {
    repo_path: crate::domain::path::RepoRelativePath,
    store_path: crate::domain::path::StoreRelativePath,
}
fn post_paths(
    _store_root: &Path,
    record: &RecoveryRecord,
    operation: &OperationRecord,
) -> Result<PostPaths> {
    let post = operation
        .post_state
        .as_ref()
        .ok_or_else(|| blocked(record, "move record lacks destination observations"))?;
    Ok(PostPaths {
        repo_path: post
            .repo_path
            .clone()
            .ok_or_else(|| blocked(record, "move destination repo path missing"))?,
        store_path: post
            .store_path
            .clone()
            .ok_or_else(|| blocked(record, "move destination store path missing"))?,
    })
}
fn backup_path(
    store_root: &Path,
    record: &RecoveryRecord,
    operation: &OperationRecord,
) -> Result<PathBuf> {
    let backup = operation
        .backup
        .as_ref()
        .ok_or_else(|| blocked(record, "lifecycle backup missing"))?;
    match &backup.location {
        crate::domain::operation_record::ArtifactLocation::Store { path } => {
            Ok(store_root.join(path.as_str()))
        }
        _ => Err(blocked(record, "lifecycle backup is not store-side")),
    }
}
fn fingerprint_is(path: &Path, expected: &RecoveryFingerprint) -> bool {
    RecoveryFingerprint::from_file(path).is_ok_and(|value| value == *expected)
}
fn is_original_restore(
    facts: &crate::fs::materializer::MaterializationFacts,
    strategy: MaterializationStrategy,
) -> bool {
    matches!(
        (strategy, facts.repo_entry_kind),
        (
            MaterializationStrategy::Symlink,
            RepoEntryKind::ManagedSymlink
        )
    )
}
fn is_materialized(
    facts: &crate::fs::materializer::MaterializationFacts,
    strategy: MaterializationStrategy,
) -> bool {
    match strategy {
        MaterializationStrategy::Symlink => facts.repo_entry_kind == RepoEntryKind::ManagedSymlink,
        MaterializationStrategy::Copy => {
            facts.repo_entry_kind == RepoEntryKind::RegularFile
                && facts.hardlink_free
                && facts.copy_content == crate::domain::materialization::CopyContentState::Equal
        }
    }
}
fn classify_restore_exclude(operation: &OperationRecord, actual: bool) -> RestoreExcludeFact {
    let before = operation.pre_state.exclude_owned.unwrap_or(false);
    let final_state = operation.pre_state.final_exclude_owned.unwrap_or(before);
    if matches!(
        operation.phase,
        OperationPhase::ExcludeUpdated | OperationPhase::PostCommitValidated
    ) && actual == final_state
    {
        RestoreExcludeFact::FinalState
    } else if actual == before {
        RestoreExcludeFact::BeforeState
    } else if actual == final_state {
        RestoreExcludeFact::FinalState
    } else {
        RestoreExcludeFact::Unexpected
    }
}
fn remove_owned_exclude(repo_root: &Path, operation: &OperationRecord) -> Result<()> {
    let Some(path) = &operation.pre_state.repo_path else {
        return Ok(());
    };
    if operation.pre_state.exclude_owned == Some(false) {
        GitInfoExclude.remove_entries(repo_root, &[path.as_str()])?;
    }
    Ok(())
}
