//! Durable recovery executor for the migrated item-add workflow.
//!
//! It is intentionally separate from `add.rs`: the add workflow records
//! intent and phases, while this module collects fresh facts and applies only
//! the typed Phase 6 recovery decision.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use crate::{
    domain::{
        materialization::MaterializationStrategy,
        operation_record::{
            OperationKind, OperationPhase, OperationRecord, RecoveryRecord, RecoveryRecordKind,
        },
        path::{RepoRelativePath, StoreRelativePath},
    },
    error::{AppError, Result},
    fs::materializer::{
        DefaultMaterializer, InspectionPurpose, MaterializationAction,
        MaterializationInspectionRequest, MaterializationLocation, Materializer, MutationJournal,
        NoArtifactJournal, RepoEntryKind,
    },
    fs::mutation_journal::AddMutationJournal,
    git,
    ignore::{GitInfoExclude, IgnoreBackend},
    policy::recovery_policy::{
        self, AddExcludeFact, AddManifestFact, AddRecoveryFacts, AddRepoFact, AddStoreFact,
        RecoveryDecision, RecoveryForward, RecoveryRollback,
    },
    storage::{layout, operation_record_store},
    store::{
        index,
        manifest::{self, Item, Manifest, OwnershipState},
    },
};

/// Attempts add recovery only when the record belongs to `current_repo_root`.
/// Returns `true` when the record was an add record owned by this repository,
/// even if recovery returned a conflict.
pub(crate) fn recover_if_owned(
    store_root: &Path,
    current_repo_root: &Path,
    record: &mut RecoveryRecord,
) -> Result<bool> {
    let owned = matches!(
        &record.record,
        RecoveryRecordKind::Operation(operation)
            if operation.operation == OperationKind::Add
                && !operation.phase.is_finalized()
                && operation.repo_root.as_path() == current_repo_root
    );
    if !owned {
        return Ok(false);
    }

    for _ in 0..6 {
        let operation = operation_from(record)?.clone();
        let facts = collect_facts(store_root, current_repo_root, record, &operation)?;
        match recovery_policy::decide(recovery_policy::RecoveryPolicyInput {
            operation: OperationKind::Add,
            phase: operation.phase,
            direction: operation.direction,
            facts: recovery_policy::RecoveryFacts::Add(facts),
        }) {
            RecoveryDecision::AdvanceRecord { to } => update_phase(store_root, record, to)?,
            RecoveryDecision::Rollback(RecoveryRollback::DeleteRecordOnly) => {
                operation_record_store::remove(store_root, &record.record_id)?;
                return Ok(true);
            }
            RecoveryDecision::Rollback(RecoveryRollback::RemoveOperationOwnedExclude) => {
                if operation.pre_state.exclude_owned != Some(true) {
                    let path = repo_path(record, &operation)?;
                    GitInfoExclude.remove_entries(current_repo_root, &[path.as_str()])?;
                }
                operation_record_store::remove(store_root, &record.record_id)?;
                return Ok(true);
            }
            RecoveryDecision::Forward(RecoveryForward::MaterializeRepo) => {
                materialize_repo(store_root, current_repo_root, record, &operation)?;
                update_phase(store_root, record, OperationPhase::RepoMaterialized)?;
            }
            RecoveryDecision::Forward(RecoveryForward::SaveManifest) => {
                save_manifest(store_root, record, &operation)?;
                update_phase(store_root, record, OperationPhase::ManifestSaved)?;
            }
            RecoveryDecision::Forward(RecoveryForward::VerifyPostconditionsAndComplete) => {
                update_phase(store_root, record, OperationPhase::PostCommitValidated)?;
                operation_record_store::remove(store_root, &record.record_id)?;
                return Ok(true);
            }
            RecoveryDecision::Conflict(conflict) => return Err(blocked(record, conflict.reason())),
            RecoveryDecision::Forward(action) => {
                return Err(blocked(
                    record,
                    format!("invalid add recovery action: {action:?}"),
                ));
            }
        }
    }

    Err(blocked(
        record,
        "add recovery exceeded its bounded phase transitions",
    ))
}

fn collect_facts(
    store_root: &Path,
    repo_root: &Path,
    record: &RecoveryRecord,
    operation: &OperationRecord,
) -> Result<AddRecoveryFacts> {
    let repo_path = repo_path(record, operation)?;
    let store_path = store_path(record, operation)?;
    let fingerprint = operation
        .pre_state
        .repo_fingerprint
        .as_ref()
        .ok_or_else(|| blocked(record, "add record has no source fingerprint"))?;
    let repo_absolute = repo_root.join(repo_path.as_str());
    let store_absolute = store_root.join(store_path.as_str());

    let store = match std::fs::symlink_metadata(&store_absolute) {
        Ok(metadata)
            if metadata.file_type().is_file() && fingerprint.matches_file(&store_absolute)? =>
        {
            AddStoreFact::Source
        }
        Ok(_) => AddStoreFact::Unexpected,
        Err(error) if error.kind() == ErrorKind::NotFound => AddStoreFact::Missing,
        Err(error) => return Err(AppError::io(&store_absolute, error)),
    };
    let repo = match std::fs::symlink_metadata(&repo_absolute) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            if store == AddStoreFact::Missing && fingerprint.matches_file(&repo_absolute)? {
                AddRepoFact::Source
            } else if store == AddStoreFact::Source
                && operation.strategy == MaterializationStrategy::Copy
                && fingerprint.matches_file(&repo_absolute)?
            {
                AddRepoFact::Materialized
            } else {
                AddRepoFact::Unexpected
            }
        }
        Ok(_) => {
            let materializer =
                DefaultMaterializer::new(repo_root.to_path_buf(), store_root.to_path_buf());
            let facts = materializer.inspect(MaterializationInspectionRequest {
                location: MaterializationLocation::new(repo_path.clone(), store_path.clone()),
                purpose: InspectionPurpose::Planning,
            })?;
            if facts.repo_entry_kind == RepoEntryKind::ManagedSymlink
                && store == AddStoreFact::Source
            {
                AddRepoFact::Materialized
            } else {
                AddRepoFact::Unexpected
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => AddRepoFact::Missing,
        Err(error) => return Err(AppError::io(&repo_absolute, error)),
    };
    let manifest = manifest_fact(store_root, record, operation, &repo_path)?;
    let excluded = GitInfoExclude.has_entry(repo_root, repo_path.as_str())?;
    let exclude = if operation.phase == OperationPhase::RecordCreated
        && excluded == operation.pre_state.exclude_owned.unwrap_or(false)
    {
        AddExcludeFact::BeforeState
    } else if excluded {
        AddExcludeFact::Present
    } else {
        AddExcludeFact::Unexpected
    };

    Ok(AddRecoveryFacts {
        repo,
        store,
        manifest,
        exclude,
    })
}

fn materialize_repo(
    store_root: &Path,
    repo_root: &Path,
    record: &mut RecoveryRecord,
    operation: &OperationRecord,
) -> Result<()> {
    let repo_path = repo_path(record, operation)?;
    let store_path = store_path(record, operation)?;
    let repo_absolute = repo_root.join(repo_path.as_str());
    if git::is_tracked(repo_root, &repo_absolute)? {
        return Err(AppError::PathIsTracked {
            path: repo_absolute,
        });
    }
    let location = MaterializationLocation::new(repo_path, store_path.clone());
    let mut materializer =
        DefaultMaterializer::new(repo_root.to_path_buf(), store_root.to_path_buf());
    if operation.strategy == MaterializationStrategy::Copy {
        let mut journal = AddMutationJournal::new(
            store_root,
            repo_root,
            &GitInfoExclude,
            record,
            repo_absolute.clone(),
            store_root.join(store_path.as_str()),
        );
        let prepared = materializer.prepare(
            MaterializationAction::Create {
                location: location.clone(),
                strategy: operation.strategy,
            },
            &mut journal,
        )?;
        let facts = materializer.inspect(MaterializationInspectionRequest {
            location,
            purpose: InspectionPurpose::PreCommit,
        })?;
        if facts.repo_entry_kind != RepoEntryKind::Missing || !facts.store_exists {
            return Err(AppError::FilesystemEntryChanged {
                path: repo_absolute,
            });
        }
        let permit = journal
            .issue_commit_permit(facts.write_precondition_guard(prepared.commit_context()))?;
        materializer.commit(prepared, permit)?;
        journal.cleanup_all()?;
    } else {
        let mut journal = NoArtifactJournal;
        let prepared = materializer.prepare(
            MaterializationAction::Create {
                location: location.clone(),
                strategy: operation.strategy,
            },
            &mut journal,
        )?;
        let facts = materializer.inspect(MaterializationInspectionRequest {
            location,
            purpose: InspectionPurpose::PreCommit,
        })?;
        if facts.repo_entry_kind != RepoEntryKind::Missing || !facts.store_exists {
            return Err(AppError::FilesystemEntryChanged {
                path: repo_absolute,
            });
        }
        let permit = journal
            .issue_commit_permit(facts.write_precondition_guard(prepared.commit_context()))?;
        materializer.commit(prepared, permit)?;
    }
    Ok(())
}

fn save_manifest(
    store_root: &Path,
    record: &RecoveryRecord,
    operation: &OperationRecord,
) -> Result<()> {
    let repo_path = repo_path(record, operation)?;
    let repo_store = repo_store(store_root, record, operation)?;
    let mut manifest = if manifest::manifest_path(&repo_store).is_file() {
        manifest::load(&repo_store)?
    } else {
        Manifest::new(operation.repo_id.clone(), record.created_at.clone())
    };
    if !manifest.contains(repo_path.as_str()) {
        let now = crate::context::now_iso8601();
        manifest.add(Item {
            item_id: ulid::Ulid::new().to_string(),
            origin_repo_id: operation.repo_id.clone(),
            path: repo_path.into_inner(),
            store_path: format!(
                "items/{}",
                operation.pre_state.repo_path.as_ref().unwrap().as_str()
            ),
            ownership_state: OwnershipState::Attached,
            created_at: now.clone(),
            updated_at: now,
        });
        manifest::save(&repo_store, &manifest)?;
    }
    Ok(())
}

fn manifest_fact(
    store_root: &Path,
    record: &RecoveryRecord,
    operation: &OperationRecord,
    repo_path: &RepoRelativePath,
) -> Result<AddManifestFact> {
    let repo_store = repo_store(store_root, record, operation)?;
    if !manifest::manifest_path(&repo_store).is_file() {
        return Ok(AddManifestFact::Absent);
    }
    let manifest = manifest::load(&repo_store)?;
    Ok(if manifest.contains(repo_path.as_str()) {
        AddManifestFact::Present
    } else {
        AddManifestFact::Absent
    })
}

fn repo_store(
    store_root: &Path,
    record: &RecoveryRecord,
    operation: &OperationRecord,
) -> Result<PathBuf> {
    let index = index::load(store_root)?;
    let entry = index.get(&operation.repo_id).ok_or_else(|| {
        blocked(
            record,
            "operation repository is absent from the store index",
        )
    })?;
    Ok(layout::repo_store_path(store_root, &entry.repo_store_dir))
}

fn repo_path(record: &RecoveryRecord, operation: &OperationRecord) -> Result<RepoRelativePath> {
    operation
        .pre_state
        .repo_path
        .clone()
        .ok_or_else(|| blocked(record, "operation has no repository path"))
}

fn store_path(record: &RecoveryRecord, operation: &OperationRecord) -> Result<StoreRelativePath> {
    operation
        .pre_state
        .store_path
        .clone()
        .ok_or_else(|| blocked(record, "operation has no store path"))
}

fn operation_from(record: &RecoveryRecord) -> Result<&OperationRecord> {
    match &record.record {
        RecoveryRecordKind::Operation(operation) => Ok(operation),
        RecoveryRecordKind::Artifact(_) => {
            Err(AppError::Internal("expected an operation record".into()))
        }
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
