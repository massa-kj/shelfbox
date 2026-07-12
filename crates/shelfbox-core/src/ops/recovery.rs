//! Conservative Phase 5 recovery gate.
//!
//! The durable store is usable before operation-specific truth tables land in
//! Phase 6. Until then, only two recovery actions are safe without guessing:
//! identity-matched artifact cleanup and deletion of a stale record already
//! marked `post_commit_validated`. Every other unfinished operation blocks new
//! mutation and remains intact for the Phase 6 policy engine.

use std::{collections::BTreeSet, path::Path};

use crate::{
    domain::operation_record::{
        ArtifactLocation, OperationRecord, RecoveryRecord, RecoveryRecordKind,
    },
    error::{AppError, Result},
    ignore::{GitInfoExclude, IgnoreBackend},
    storage::operation_record_store::{self, ArtifactCleanup},
};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct RecoveryReport {
    pub cleaned_artifacts: Vec<String>,
    pub cleaned_stale_operations: Vec<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ReadOnlyRecoveryStatus {
    pub record_ids: Vec<String>,
    pub affected_item_paths: BTreeSet<String>,
    pub has_unattributed_record: bool,
}

/// Runs recovery while the caller holds the store write lock. All records are
/// loaded and validated before the first cleanup, which guarantees malformed
/// and future-version inputs are preserved rather than partially processed.
pub(crate) fn recover_before_mutation(
    store_root: &Path,
    current_repo_root: &Path,
) -> Result<RecoveryReport> {
    let records = operation_record_store::load_all(store_root)?;
    let mut report = RecoveryReport::default();

    for record in records {
        match &record.record {
            RecoveryRecordKind::Artifact(artifact) => {
                let cleanup = operation_record_store::cleanup_artifact(
                    store_root,
                    current_repo_root,
                    &record.record_id,
                    artifact,
                )?;
                cleanup_repo_temp_exclude(current_repo_root, artifact)?;
                operation_record_store::remove(store_root, &record.record_id)?;
                if matches!(
                    cleanup,
                    ArtifactCleanup::Removed | ArtifactCleanup::AlreadyAbsent
                ) {
                    report.cleaned_artifacts.push(record.record_id);
                }
            }
            RecoveryRecordKind::Operation(operation) if operation.phase.is_finalized() => {
                // A durable post-commit marker means the user-visible state
                // already passed final validation. Never roll it back merely
                // because the final record deletion did not persist.
                operation_record_store::remove(store_root, &record.record_id)?;
                report.cleaned_stale_operations.push(record.record_id);
            }
            RecoveryRecordKind::Operation(operation) => {
                return Err(unfinished_operation_error(&record, operation));
            }
        }
    }

    Ok(report)
}

/// Store-scoped mutating commands (for example GC) do not have a repository
/// root. They may safely clean only store-side artifacts and stale completed
/// records; any repo-side artifact or unfinished operation blocks the command.
pub(crate) fn recover_store_before_mutation(store_root: &Path) -> Result<RecoveryReport> {
    let records = operation_record_store::load_all(store_root)?;
    let mut report = RecoveryReport::default();

    for record in records {
        match &record.record {
            RecoveryRecordKind::Artifact(artifact)
                if matches!(artifact.location, ArtifactLocation::Store { .. }) =>
            {
                let cleanup = operation_record_store::cleanup_artifact(
                    store_root,
                    Path::new(""),
                    &record.record_id,
                    artifact,
                )?;
                operation_record_store::remove(store_root, &record.record_id)?;
                if matches!(
                    cleanup,
                    ArtifactCleanup::Removed | ArtifactCleanup::AlreadyAbsent
                ) {
                    report.cleaned_artifacts.push(record.record_id);
                }
            }
            RecoveryRecordKind::Operation(operation) if operation.phase.is_finalized() => {
                operation_record_store::remove(store_root, &record.record_id)?;
                report.cleaned_stale_operations.push(record.record_id);
            }
            RecoveryRecordKind::Operation(operation) => {
                return Err(unfinished_operation_error(&record, operation));
            }
            RecoveryRecordKind::Artifact(_) => {
                return Err(AppError::RecoveryBlocked {
                    record_id: record.record_id,
                    reason: "repo-side artifact requires recovery from its owning repository"
                        .into(),
                });
            }
        }
    }

    Ok(report)
}

/// Inspects records without writing, for status/reporting paths.
pub(crate) fn read_only_status(
    store_root: &Path,
    repo_root: &Path,
) -> Result<ReadOnlyRecoveryStatus> {
    let mut status = ReadOnlyRecoveryStatus::default();
    for record in operation_record_store::load_all(store_root)? {
        if record.is_finalized_operation() {
            // It is stale but safe; write contexts will clean it. Read-only
            // status reports only unfinished work that can block mutation.
            continue;
        }
        status.record_ids.push(record.record_id.clone());
        match record.record {
            RecoveryRecordKind::Operation(operation) => {
                if operation.repo_root.as_path() == repo_root {
                    if let Some(path) = operation.pre_state.repo_path {
                        status.affected_item_paths.insert(path.into_inner());
                    } else {
                        status.has_unattributed_record = true;
                    }
                } else {
                    status.has_unattributed_record = true;
                }
            }
            RecoveryRecordKind::Artifact(artifact) => match artifact.location {
                ArtifactLocation::Repo {
                    repo_root: owner,
                    path,
                } if owner.as_path() == repo_root => {
                    status.affected_item_paths.insert(path.into_inner());
                }
                _ => status.has_unattributed_record = true,
            },
        }
    }
    Ok(status)
}

fn cleanup_repo_temp_exclude(
    current_repo_root: &Path,
    artifact: &crate::domain::operation_record::ArtifactRecord,
) -> Result<()> {
    let Some(exclude) = &artifact.repo_temp_exclude else {
        return Ok(());
    };
    if !exclude.added_by_operation {
        return Ok(());
    }
    let ArtifactLocation::Repo { repo_root, .. } = &artifact.location else {
        return Ok(());
    };
    if repo_root.as_path() != current_repo_root {
        return Ok(());
    }
    GitInfoExclude.remove_entries(current_repo_root, &[exclude.path.as_str()])
}

fn unfinished_operation_error(record: &RecoveryRecord, operation: &OperationRecord) -> AppError {
    AppError::RecoveryBlocked {
        record_id: record.record_id.clone(),
        reason: format!(
            "unfinished {:?} operation at phase {:?}; Phase 6 recovery policy is required",
            operation.operation, operation.phase
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{
        context,
        domain::{
            copy_safety::ArtifactScope,
            materialization::MaterializationStrategy,
            operation_record::{
                ArtifactLocation, ArtifactRecord, ArtifactState, OperationKind, OperationPhase,
                OperationPreState, OperationRecord, RecoveryAbsolutePath, RecoveryRecord,
                RecoveryRecordKind, OPERATION_RECORD_SCHEMA_VERSION,
            },
        },
        storage::operation_record_store,
    };

    use super::*;

    fn root(path: &Path) -> RecoveryAbsolutePath {
        RecoveryAbsolutePath::new(path).unwrap()
    }

    fn operation(root_path: &Path, phase: OperationPhase) -> RecoveryRecord {
        RecoveryRecord {
            schema_version: OPERATION_RECORD_SCHEMA_VERSION,
            record_id: ulid::Ulid::new().to_string(),
            created_at: context::now_iso8601(),
            record: RecoveryRecordKind::Operation(OperationRecord {
                operation: OperationKind::Add,
                phase,
                repo_id: "repo-1".into(),
                repo_root: root(root_path),
                strategy: MaterializationStrategy::Copy,
                direction: None,
                pre_state: OperationPreState {
                    repo_path: Some("secret.env".parse().unwrap()),
                    ..OperationPreState::default()
                },
                artifact_record_ids: Vec::new(),
                backup: None,
            }),
        }
    }

    #[test]
    fn unfinished_operation_blocks_mutation_without_deleting_record() {
        let store = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let record = operation(repo.path(), OperationPhase::RecordCreated);
        operation_record_store::create(store.path(), &record).unwrap();

        assert!(matches!(
            recover_before_mutation(store.path(), repo.path()),
            Err(AppError::RecoveryBlocked { .. })
        ));
        assert_eq!(
            operation_record_store::load_all(store.path()).unwrap(),
            vec![record]
        );
    }

    #[test]
    fn stale_final_operation_is_cleaned_without_rollback() {
        let store = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let record = operation(repo.path(), OperationPhase::PostCommitValidated);
        operation_record_store::create(store.path(), &record).unwrap();

        let report = recover_before_mutation(store.path(), repo.path()).unwrap();
        assert_eq!(report.cleaned_stale_operations, vec![record.record_id]);
        assert!(operation_record_store::load_all(store.path())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn read_only_status_reports_unfinished_record_without_cleaning_it() {
        let store = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let record = operation(repo.path(), OperationPhase::RecordCreated);
        operation_record_store::create(store.path(), &record).unwrap();

        let status = read_only_status(store.path(), repo.path()).unwrap();
        assert_eq!(status.record_ids, vec![record.record_id.clone()]);
        assert!(status.affected_item_paths.contains("secret.env"));
        assert_eq!(
            operation_record_store::load_all(store.path()).unwrap(),
            vec![record]
        );
    }

    #[test]
    fn artifact_record_cleanup_removes_only_the_recorded_temp() {
        let store = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let path = repo.path().join(".shelfbox-temp");
        std::fs::write(&path, "empty before authorization").unwrap();
        let identity = operation_record_store::identity_from_path(&path).unwrap();
        let record = RecoveryRecord {
            schema_version: OPERATION_RECORD_SCHEMA_VERSION,
            record_id: ulid::Ulid::new().to_string(),
            created_at: context::now_iso8601(),
            record: RecoveryRecordKind::Artifact(ArtifactRecord {
                repo_id: "repo-1".into(),
                scope: ArtifactScope::RepoSide,
                location: ArtifactLocation::Repo {
                    repo_root: root(repo.path()),
                    path: ".shelfbox-temp".parse().unwrap(),
                },
                state: ArtifactState::Created {
                    identity,
                    plaintext_authorized: false,
                },
                repo_temp_exclude: None,
            }),
        };
        operation_record_store::create(store.path(), &record).unwrap();

        let report = recover_before_mutation(store.path(), repo.path()).unwrap();
        assert_eq!(report.cleaned_artifacts, vec![record.record_id]);
        assert!(!path.exists());
    }

    #[test]
    fn recovery_removes_only_the_exact_repo_temp_exclude_it_owns() {
        let store = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".git/info")).unwrap();
        let path = repo.path().join(".shelfbox-temp");
        std::fs::write(&path, "empty before authorization").unwrap();
        let identity = operation_record_store::identity_from_path(&path).unwrap();
        GitInfoExclude
            .add_entries(repo.path(), &[".shelfbox-temp"])
            .unwrap();
        let record = RecoveryRecord {
            schema_version: OPERATION_RECORD_SCHEMA_VERSION,
            record_id: ulid::Ulid::new().to_string(),
            created_at: context::now_iso8601(),
            record: RecoveryRecordKind::Artifact(ArtifactRecord {
                repo_id: "repo-1".into(),
                scope: ArtifactScope::RepoSide,
                location: ArtifactLocation::Repo {
                    repo_root: root(repo.path()),
                    path: ".shelfbox-temp".parse().unwrap(),
                },
                state: ArtifactState::Created {
                    identity,
                    plaintext_authorized: false,
                },
                repo_temp_exclude: Some(crate::domain::operation_record::RepoTempExclude {
                    path: ".shelfbox-temp".parse().unwrap(),
                    added_by_operation: true,
                    verified: true,
                }),
            }),
        };
        operation_record_store::create(store.path(), &record).unwrap();

        recover_before_mutation(store.path(), repo.path()).unwrap();

        assert!(!path.exists());
        assert!(!GitInfoExclude
            .has_entry(repo.path(), ".shelfbox-temp")
            .unwrap());
    }
}
