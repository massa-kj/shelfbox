//! Durable storage for operation and recovery-artifact records.
//!
//! All mutation methods use a private directory, private files, file fsync,
//! and parent-directory fsync. Loading performs schema and path validation
//! before returning any record, so recovery never acts on malformed input.

use std::{
    collections::BTreeSet,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{
    domain::operation_record::{
        validate_record_id, ArtifactLocation, ArtifactRecord, ArtifactState,
        RecoveryBackupMetadata, RecoveryFileIdentity, RecoveryRecord, RecoveryRecordKind,
        LEGACY_OPERATION_RECORD_SCHEMA_VERSION, OPERATION_RECORD_SCHEMA_VERSION,
    },
    error::{AppError, Result},
    failpoint::{self, Failpoint},
    fs::{platform, secure_transfer},
    storage::{
        atomic_write::{
            self, mutation_sync_mode, sync_directory, AtomicWriteOptions, FileSyncMode,
        },
        layout,
    },
};

pub(crate) fn records_dir(store_root: &Path) -> PathBuf {
    layout::operation_records_dir(store_root)
}

pub(crate) fn record_path(store_root: &Path, record_id: &str) -> Result<PathBuf> {
    validate_record_id(record_id).map_err(|reason| AppError::OperationRecordMalformed {
        path: records_dir(store_root),
        reason,
    })?;
    Ok(records_dir(store_root).join(format!("{record_id}.json")))
}

/// Creates a previously absent record. The caller must hold the store write
/// lock. An existing record is never overwritten by this method.
#[allow(dead_code)] // Consumed by the durable mutation journal in Phase 7.
pub(crate) fn create(store_root: &Path, record: &RecoveryRecord) -> Result<()> {
    validate_record(record, &records_dir(store_root))?;
    if record.schema_version != OPERATION_RECORD_SCHEMA_VERSION {
        return Err(AppError::OperationRecordMalformed {
            path: records_dir(store_root),
            reason: "new operation records must use schema version 2".into(),
        });
    }
    let path = record_path(store_root, &record.record_id)?;
    if path.exists() {
        return Err(AppError::OperationRecordMalformed {
            path,
            reason: "refusing to overwrite an existing operation record".into(),
        });
    }
    write_record(&path, record)?;
    failpoint::after(match &record.record {
        RecoveryRecordKind::Operation(_) => Failpoint::OperationRecordCreated,
        RecoveryRecordKind::Artifact(_) => Failpoint::ArtifactPathRecorded,
    })
}

/// Durably replaces an existing record after a phase or artifact-state update.
#[allow(dead_code)] // Consumed by the durable mutation journal in Phase 7.
pub(crate) fn update(store_root: &Path, record: &RecoveryRecord) -> Result<()> {
    validate_record(record, &records_dir(store_root))?;
    let path = record_path(store_root, &record.record_id)?;
    if !path.is_file() {
        return Err(AppError::OperationRecordMalformed {
            path,
            reason: "cannot update a missing operation record".into(),
        });
    }
    // Updating a legacy strict record is a safe schema migration: v1 could
    // only have been written with the historical strict contract, and v2
    // makes that fact explicit for later readers.
    let mut upgraded = record.clone();
    if upgraded.schema_version == LEGACY_OPERATION_RECORD_SCHEMA_VERSION {
        upgraded.schema_version = OPERATION_RECORD_SCHEMA_VERSION;
    }
    write_record(&path, &upgraded)?;
    failpoint::after(match &record.record {
        RecoveryRecordKind::Operation(operation) => {
            Failpoint::OperationPhaseUpdated(operation.phase)
        }
        RecoveryRecordKind::Artifact(ArtifactRecord {
            state: ArtifactState::Created { .. },
            ..
        }) => Failpoint::TempIdentityRecorded,
        RecoveryRecordKind::Artifact(ArtifactRecord {
            state: ArtifactState::Planned,
            ..
        }) => Failpoint::ArtifactPathRecorded,
    })
}

/// Deletes a completed record and durably syncs the containing directory.
/// A record already absent after a crash is considered successfully deleted.
pub(crate) fn remove(store_root: &Path, record_id: &str) -> Result<()> {
    let path = record_path(store_root, record_id)?;
    let durability = match record_durability(store_root, record_id) {
        Ok(durability) => durability,
        Err(AppError::Io { source, .. }) if source.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    match fs::remove_file(&path) {
        Ok(()) => {
            sync_records_dir(store_root, durability)?;
            failpoint::after(Failpoint::RecordDeleted)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::io(path, error)),
    }
}

/// Loads every durable record in deterministic record-id order. A malformed or
/// unsupported record aborts the whole load before recovery can mutate any
/// artifact, leaving every input file untouched.
pub(crate) fn load_all(store_root: &Path) -> Result<Vec<RecoveryRecord>> {
    let dir = records_dir(store_root);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(AppError::io(dir, error)),
    };

    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| AppError::io(&dir, error))?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
            paths.push(path);
        }
    }
    paths.sort();

    let mut records = Vec::with_capacity(paths.len());
    for path in paths {
        let record_id = path
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or_else(|| AppError::OperationRecordMalformed {
                path: path.clone(),
                reason: "record file name must be valid UTF-8".into(),
            })?;
        validate_record_id(record_id).map_err(|reason| AppError::OperationRecordMalformed {
            path: path.clone(),
            reason,
        })?;

        let contents = fs::read_to_string(&path).map_err(|error| AppError::io(&path, error))?;
        let version: RecordVersion = serde_json::from_str(&contents).map_err(|error| {
            AppError::OperationRecordMalformed {
                path: path.clone(),
                reason: error.to_string(),
            }
        })?;
        if !matches!(
            version.schema_version,
            LEGACY_OPERATION_RECORD_SCHEMA_VERSION | OPERATION_RECORD_SCHEMA_VERSION
        ) {
            return Err(AppError::OperationRecordUnsupportedVersion {
                path,
                found: version.schema_version,
                supported: OPERATION_RECORD_SCHEMA_VERSION,
            });
        }

        let record: RecoveryRecord = serde_json::from_str(&contents).map_err(|error| {
            AppError::OperationRecordMalformed {
                path: path.clone(),
                reason: error.to_string(),
            }
        })?;
        validate_record(&record, &path)?;
        if record.record_id != record_id {
            return Err(AppError::OperationRecordMalformed {
                path,
                reason: "record_id does not match the file name".into(),
            });
        }
        records.push(record);
    }

    records.sort_by(|left, right| left.record_id.cmp(&right.record_id));
    Ok(records)
}

/// Returns every store-root-relative path named by an unfinished record. GC
/// must preserve both a candidate containing a protected path and a candidate
/// that is itself contained by one.
pub(crate) fn protected_store_paths(store_root: &Path) -> Result<BTreeSet<PathBuf>> {
    let mut paths = BTreeSet::new();
    for record in load_all(store_root)? {
        if record.is_finalized_operation() {
            continue;
        }
        match record.record {
            RecoveryRecordKind::Operation(operation) => {
                if let Some(path) = operation.pre_state.store_path {
                    paths.insert(store_root.join(path.as_str()));
                }
                if let Some(post_state) = operation.post_state {
                    if let Some(path) = post_state.store_path {
                        paths.insert(store_root.join(path.as_str()));
                    }
                }
                if let Some(backup) = operation.backup {
                    if let ArtifactLocation::Store { path } = backup.location {
                        paths.insert(store_root.join(path.as_str()));
                    }
                }
            }
            RecoveryRecordKind::Artifact(artifact) => {
                if let ArtifactLocation::Store { path } = artifact.location {
                    paths.insert(store_root.join(path.as_str()));
                }
            }
        }
    }
    Ok(paths)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArtifactCleanup {
    Removed,
    AlreadyAbsent,
}

/// Cleans an artifact only when its record is sufficient to prove ownership.
/// A planned artifact that unexpectedly exists, a root mismatch, an unsafe
/// parent, or a different identity is reported as conflict and never removed.
pub(crate) fn cleanup_artifact(
    store_root: &Path,
    current_repo_root: &Path,
    record_id: &str,
    artifact: &ArtifactRecord,
) -> Result<ArtifactCleanup> {
    let path = artifact_path(store_root, current_repo_root, record_id, &artifact.location)?;
    let entry = match platform::inspect_no_follow(&path) {
        Ok(entry) => entry,
        Err(AppError::Io { source, .. }) if source.kind() == ErrorKind::NotFound => {
            return Ok(ArtifactCleanup::AlreadyAbsent);
        }
        Err(error) => return Err(error),
    };

    let ArtifactState::Created { identity, .. } = &artifact.state else {
        return Err(AppError::RecoveryArtifactConflict {
            record_id: record_id.into(),
            path,
            reason: "a path reserved before creation now exists".into(),
        });
    };
    if !identity_matches(identity, entry.identity.volume, entry.identity.file) {
        return Err(AppError::RecoveryArtifactConflict {
            record_id: record_id.into(),
            path,
            reason: "artifact identity no longer matches the durable record".into(),
        });
    }

    let durability = record_durability(store_root, record_id)?;
    fs::remove_file(&path).map_err(|error| AppError::io(&path, error))?;
    let parent = path
        .parent()
        .ok_or_else(|| AppError::RecoveryArtifactConflict {
            record_id: record_id.into(),
            path: path.clone(),
            reason: "artifact has no parent directory".into(),
        })?;
    sync_directory(parent, mutation_sync_mode(durability))?;
    Ok(ArtifactCleanup::Removed)
}

/// Removes an operation-owned recovery backup only when the durable record
/// still names the exact file identity.  Lifecycle recovery uses this instead
/// of a path-based `remove_file`, so a user-created replacement is preserved
/// and reported as a conflict.
pub(crate) fn cleanup_backup(
    store_root: &Path,
    current_repo_root: &Path,
    record_id: &str,
    backup: &RecoveryBackupMetadata,
) -> Result<ArtifactCleanup> {
    let path = artifact_path(store_root, current_repo_root, record_id, &backup.location)?;
    let entry = match platform::inspect_no_follow(&path) {
        Ok(entry) => entry,
        Err(AppError::Io { source, .. }) if source.kind() == ErrorKind::NotFound => {
            return Ok(ArtifactCleanup::AlreadyAbsent);
        }
        Err(error) => return Err(error),
    };
    let Some(identity) = &backup.expected_identity else {
        return Err(AppError::RecoveryArtifactConflict {
            record_id: record_id.into(),
            path,
            reason: "recovery backup exists without a durable identity".into(),
        });
    };
    if !identity_matches(identity, entry.identity.volume, entry.identity.file) {
        return Err(AppError::RecoveryArtifactConflict {
            record_id: record_id.into(),
            path,
            reason: "recovery backup identity no longer matches the durable record".into(),
        });
    }
    let durability = record_durability(store_root, record_id)?;
    fs::remove_file(&path).map_err(|error| AppError::io(&path, error))?;
    let parent = path
        .parent()
        .ok_or_else(|| AppError::RecoveryArtifactConflict {
            record_id: record_id.into(),
            path: path.clone(),
            reason: "recovery backup has no parent directory".into(),
        })?;
    sync_directory(parent, mutation_sync_mode(durability))?;
    Ok(ArtifactCleanup::Removed)
}

pub(crate) fn identity_from_parts(volume: u64, file: [u8; 16]) -> RecoveryFileIdentity {
    let mut file_hex = String::with_capacity(32);
    for byte in file {
        use std::fmt::Write;
        let _ = write!(&mut file_hex, "{byte:02x}");
    }
    RecoveryFileIdentity::new(volume, file_hex).expect("platform file identity has fixed width")
}

/// Captures the opaque no-follow identity that must be written to an artifact
/// record before the temp can receive plaintext.
#[allow(dead_code)] // Consumed by the durable mutation journal in Phase 7.
pub(crate) fn identity_from_path(path: &Path) -> Result<RecoveryFileIdentity> {
    let entry = platform::inspect_no_follow(path)?;
    Ok(identity_from_parts(
        entry.identity.volume,
        entry.identity.file,
    ))
}

fn artifact_path(
    store_root: &Path,
    current_repo_root: &Path,
    record_id: &str,
    location: &ArtifactLocation,
) -> Result<PathBuf> {
    match location {
        ArtifactLocation::Repo { repo_root, path } => {
            if repo_root.as_path() != current_repo_root {
                return Err(AppError::RecoveryArtifactConflict {
                    record_id: record_id.into(),
                    path: repo_root.as_path().join(path.as_str()),
                    reason: "artifact belongs to a different repository root".into(),
                });
            }
            let path = current_repo_root.join(path.as_str());
            secure_transfer::validate_parent_path(current_repo_root, &path)?;
            Ok(path)
        }
        ArtifactLocation::Store { path } => {
            let path = store_root.join(path.as_str());
            secure_transfer::validate_parent_path(store_root, &path)?;
            Ok(path)
        }
    }
}

fn identity_matches(expected: &RecoveryFileIdentity, volume: u64, file: [u8; 16]) -> bool {
    expected == &identity_from_parts(volume, file)
}

#[allow(dead_code)] // Reached through create/update once a journal writes records in Phase 7.
fn write_record(path: &Path, record: &RecoveryRecord) -> Result<()> {
    let json = serde_json::to_vec_pretty(record).map_err(|error| AppError::json(path, error))?;
    atomic_write::write_with_options(
        path,
        json,
        AtomicWriteOptions::new(atomic_write::ParentDirMode::Private)
            .with_file_sync(FileSyncMode::BeforeRename)
            .with_parent_directory_sync(mutation_sync_mode(record.durability)),
    )
}

fn sync_records_dir(
    store_root: &Path,
    durability: crate::domain::mutation_durability::MutationDurability,
) -> Result<()> {
    sync_directory(&records_dir(store_root), mutation_sync_mode(durability))
}

fn record_durability(
    store_root: &Path,
    record_id: &str,
) -> Result<crate::domain::mutation_durability::MutationDurability> {
    let path = record_path(store_root, record_id)?;
    let contents = fs::read_to_string(&path).map_err(|error| AppError::io(&path, error))?;
    let record: RecoveryRecord =
        serde_json::from_str(&contents).map_err(|error| AppError::OperationRecordMalformed {
            path,
            reason: error.to_string(),
        })?;
    Ok(record.durability)
}

fn validate_record(record: &RecoveryRecord, path: &Path) -> Result<()> {
    record
        .validate()
        .map_err(|reason| AppError::OperationRecordMalformed {
            path: path.to_path_buf(),
            reason,
        })
}

#[derive(Deserialize)]
struct RecordVersion {
    schema_version: u32,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{
        domain::{
            copy_safety::ArtifactScope,
            materialization::MaterializationStrategy,
            operation_record::{
                ArtifactLocation, ArtifactRecord, ArtifactState, OperationKind, OperationPhase,
                OperationPreState, OperationRecord, RecoveryAbsolutePath, RecoveryRecordKind,
            },
        },
        failpoint::{self, Failpoint},
        fs::platform,
    };

    use super::*;

    fn record_id() -> String {
        ulid::Ulid::new().to_string()
    }

    fn repo_root(path: &Path) -> RecoveryAbsolutePath {
        RecoveryAbsolutePath::new(path).unwrap()
    }

    fn operation_record(id: String, root: &Path) -> RecoveryRecord {
        RecoveryRecord {
            schema_version: OPERATION_RECORD_SCHEMA_VERSION,
            durability: crate::domain::mutation_durability::MutationDurability::Require,
            record_id: id,
            created_at: "2026-07-12T00:00:00Z".into(),
            record: RecoveryRecordKind::Operation(OperationRecord {
                operation: OperationKind::Add,
                phase: OperationPhase::RecordCreated,
                repo_id: "repo-1".into(),
                repo_root: repo_root(root),
                repo_store_path: None,
                strategy: MaterializationStrategy::Copy,
                direction: None,
                pre_state: OperationPreState {
                    store_path: Some("repos/project/items/secret.env".parse().unwrap()),
                    ..OperationPreState::default()
                },
                post_state: None,
                artifact_record_ids: Vec::new(),
                backup: None,
            }),
        }
    }

    fn artifact_record(id: String, root: &Path, relative: &str) -> RecoveryRecord {
        RecoveryRecord {
            schema_version: OPERATION_RECORD_SCHEMA_VERSION,
            durability: crate::domain::mutation_durability::MutationDurability::Require,
            record_id: id,
            created_at: "2026-07-12T00:00:00Z".into(),
            record: RecoveryRecordKind::Artifact(ArtifactRecord {
                repo_id: "repo-1".into(),
                scope: ArtifactScope::RepoSide,
                location: ArtifactLocation::Repo {
                    repo_root: repo_root(root),
                    path: relative.parse().unwrap(),
                },
                state: ArtifactState::Planned,
                repo_temp_exclude: None,
            }),
        }
    }

    #[test]
    fn create_update_delete_are_atomic_and_idempotent() {
        let store = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let mut record = operation_record(record_id(), repo.path());

        create(store.path(), &record).unwrap();
        assert_eq!(load_all(store.path()).unwrap(), vec![record.clone()]);

        let RecoveryRecordKind::Operation(operation) = &mut record.record else {
            unreachable!()
        };
        operation.phase = OperationPhase::PostCommitValidated;
        update(store.path(), &record).unwrap();
        assert!(load_all(store.path()).unwrap()[0].is_finalized_operation());

        remove(store.path(), &record.record_id).unwrap();
        remove(store.path(), &record.record_id).unwrap();
        assert!(load_all(store.path()).unwrap().is_empty());
    }

    #[test]
    fn failpoints_stop_after_the_record_or_phase_is_already_durable() {
        let store = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let mut record = operation_record(record_id(), repo.path());

        let create_id = record.record_id.clone();
        let create_hook = failpoint::install_test_hook(|point| {
            if *point == Failpoint::OperationRecordCreated {
                return Err(AppError::Internal("test interruption".into()));
            }
            Ok(())
        });
        assert!(matches!(
            create(store.path(), &record),
            Err(AppError::Internal(_))
        ));
        drop(create_hook);
        assert_eq!(load_all(store.path()).unwrap()[0].record_id, create_id);

        let RecoveryRecordKind::Operation(operation) = &mut record.record else {
            unreachable!()
        };
        operation.phase = OperationPhase::ManifestSaved;
        let update_hook = failpoint::install_test_hook(|point| {
            if *point == Failpoint::OperationPhaseUpdated(OperationPhase::ManifestSaved) {
                return Err(AppError::Internal("test interruption".into()));
            }
            Ok(())
        });
        assert!(matches!(
            update(store.path(), &record),
            Err(AppError::Internal(_))
        ));
        drop(update_hook);

        assert!(matches!(
            load_all(store.path()).unwrap()[0].record,
            RecoveryRecordKind::Operation(OperationRecord {
                phase: OperationPhase::ManifestSaved,
                ..
            })
        ));
    }

    #[test]
    fn malformed_and_future_records_are_rejected_without_deletion() {
        let store = tempfile::tempdir().unwrap();
        let dir = records_dir(store.path());
        std::fs::create_dir_all(&dir).unwrap();
        let id = record_id();
        let path = dir.join(format!("{id}.json"));
        std::fs::write(&path, "{not json").unwrap();
        assert!(matches!(
            load_all(store.path()),
            Err(AppError::OperationRecordMalformed { .. })
        ));
        assert!(path.exists());

        std::fs::write(&path, r#"{"schema_version":999}"#).unwrap();
        assert!(matches!(
            load_all(store.path()),
            Err(AppError::OperationRecordUnsupportedVersion { found: 999, .. })
        ));
        assert!(path.exists());
    }

    #[test]
    fn v1_records_normalize_to_require_and_v2_requires_valid_durability() {
        let store = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let record = operation_record(record_id(), repo.path());
        let dir = records_dir(store.path());
        std::fs::create_dir_all(&dir).unwrap();
        let path = record_path(store.path(), &record.record_id).unwrap();

        let mut v1 = serde_json::to_value(&record).unwrap();
        v1["schema_version"] = serde_json::json!(LEGACY_OPERATION_RECORD_SCHEMA_VERSION);
        v1.as_object_mut().unwrap().remove("durability");
        std::fs::write(&path, serde_json::to_vec(&v1).unwrap()).unwrap();

        let loaded = load_all(store.path()).unwrap();
        assert_eq!(
            loaded[0].schema_version,
            LEGACY_OPERATION_RECORD_SCHEMA_VERSION
        );
        assert_eq!(
            loaded[0].durability,
            crate::domain::mutation_durability::MutationDurability::Require
        );

        let mut invalid_v2 = serde_json::to_value(&record).unwrap();
        invalid_v2["durability"] = serde_json::json!("anything-goes");
        std::fs::write(&path, serde_json::to_vec(&invalid_v2).unwrap()).unwrap();
        assert!(matches!(
            load_all(store.path()),
            Err(AppError::OperationRecordMalformed { .. })
        ));
    }

    #[test]
    fn legacy_reader_fixture_refuses_v2_records() {
        let record = serde_json::json!({"schema_version": OPERATION_RECORD_SCHEMA_VERSION});
        let version: RecordVersion = serde_json::from_value(record).unwrap();
        let legacy_reader = |found| {
            (found == LEGACY_OPERATION_RECORD_SCHEMA_VERSION)
                .then_some(())
                .ok_or(found)
        };
        assert_eq!(
            legacy_reader(version.schema_version),
            Err(OPERATION_RECORD_SCHEMA_VERSION)
        );
    }

    #[test]
    fn updating_a_legacy_strict_record_migrates_it_to_v2() {
        let store = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let record = operation_record(record_id(), repo.path());
        let dir = records_dir(store.path());
        std::fs::create_dir_all(&dir).unwrap();
        let path = record_path(store.path(), &record.record_id).unwrap();
        let mut v1 = serde_json::to_value(&record).unwrap();
        v1["schema_version"] = serde_json::json!(LEGACY_OPERATION_RECORD_SCHEMA_VERSION);
        v1.as_object_mut().unwrap().remove("durability");
        std::fs::write(&path, serde_json::to_vec(&v1).unwrap()).unwrap();

        let loaded = load_all(store.path()).unwrap().pop().unwrap();
        update(store.path(), &loaded).unwrap();
        let persisted: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(persisted["schema_version"], OPERATION_RECORD_SCHEMA_VERSION);
        assert_eq!(persisted["durability"], "require");
    }

    #[test]
    fn path_escape_is_rejected_on_load() {
        let store = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let record = artifact_record(record_id(), repo.path(), ".shelfbox-temp");
        create(store.path(), &record).unwrap();
        let path = record_path(store.path(), &record.record_id).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        value["record"]["location"]["path"] = serde_json::json!("../escape");
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

        assert!(matches!(
            load_all(store.path()),
            Err(AppError::OperationRecordMalformed { .. })
        ));
    }

    #[test]
    fn planned_artifact_that_is_absent_cleans_idempotently() {
        let store = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let record = artifact_record(record_id(), repo.path(), ".shelfbox-temp");
        let RecoveryRecordKind::Artifact(artifact) = &record.record else {
            unreachable!()
        };

        assert_eq!(
            cleanup_artifact(store.path(), repo.path(), &record.record_id, artifact).unwrap(),
            ArtifactCleanup::AlreadyAbsent
        );
    }

    #[test]
    fn replaced_artifact_is_never_deleted() {
        let store = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let path = repo.path().join(".shelfbox-temp");
        std::fs::write(&path, "first").unwrap();
        let first = platform::inspect_no_follow(&path).unwrap();
        let replacement = repo.path().join(".replacement");
        std::fs::write(&replacement, "replacement").unwrap();
        std::fs::rename(&replacement, &path).unwrap();

        let record = RecoveryRecord {
            schema_version: OPERATION_RECORD_SCHEMA_VERSION,
            durability: crate::domain::mutation_durability::MutationDurability::Require,
            record_id: record_id(),
            created_at: "2026-07-12T00:00:00Z".into(),
            record: RecoveryRecordKind::Artifact(ArtifactRecord {
                repo_id: "repo-1".into(),
                scope: ArtifactScope::RepoSide,
                location: ArtifactLocation::Repo {
                    repo_root: repo_root(repo.path()),
                    path: ".shelfbox-temp".parse().unwrap(),
                },
                state: ArtifactState::Created {
                    identity: identity_from_parts(first.identity.volume, first.identity.file),
                    plaintext_authorized: false,
                },
                repo_temp_exclude: None,
            }),
        };
        let RecoveryRecordKind::Artifact(artifact) = &record.record else {
            unreachable!()
        };

        assert!(matches!(
            cleanup_artifact(store.path(), repo.path(), &record.record_id, artifact),
            Err(AppError::RecoveryArtifactConflict { .. })
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "replacement");
    }

    #[test]
    #[cfg(unix)]
    fn records_directory_and_files_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let store = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let record = operation_record(record_id(), repo.path());
        create(store.path(), &record).unwrap();

        let dir_mode = std::fs::metadata(records_dir(store.path()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let file_mode = std::fs::metadata(record_path(store.path(), &record.record_id).unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }

    #[test]
    fn protected_store_paths_include_unfinished_operation_paths() {
        let store = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let record = operation_record(record_id(), repo.path());
        create(store.path(), &record).unwrap();

        assert!(protected_store_paths(store.path())
            .unwrap()
            .contains(&store.path().join("repos/project/items/secret.env")));
    }

    #[test]
    fn artifact_path_and_identity_are_durable_before_plaintext_write() {
        let store = tempfile::tempdir().unwrap();
        let record_id = record_id();
        let mut record = RecoveryRecord {
            schema_version: OPERATION_RECORD_SCHEMA_VERSION,
            durability: crate::domain::mutation_durability::MutationDurability::Require,
            record_id: record_id.clone(),
            created_at: "2026-07-12T00:00:00Z".into(),
            record: RecoveryRecordKind::Artifact(ArtifactRecord {
                repo_id: "repo-1".into(),
                scope: ArtifactScope::StoreSide,
                location: ArtifactLocation::Store {
                    path: "repos/project/.shelfbox-temp".parse().unwrap(),
                },
                state: ArtifactState::Planned,
                repo_temp_exclude: None,
            }),
        };

        // The path record reaches durable storage before the temp exists.
        create(store.path(), &record).unwrap();
        let temp = store.path().join("repos/project/.shelfbox-temp");
        assert!(!temp.exists());
        assert!(matches!(
            &load_all(store.path()).unwrap()[0].record,
            RecoveryRecordKind::Artifact(ArtifactRecord {
                state: ArtifactState::Planned,
                ..
            })
        ));

        std::fs::create_dir_all(temp.parent().unwrap()).unwrap();
        std::fs::File::create(&temp).unwrap();
        let entry = platform::inspect_no_follow(&temp).unwrap();
        let RecoveryRecordKind::Artifact(artifact) = &mut record.record else {
            unreachable!()
        };
        artifact.state = ArtifactState::Created {
            identity: identity_from_parts(entry.identity.volume, entry.identity.file),
            plaintext_authorized: true,
        };
        update(store.path(), &record).unwrap();

        // The empty file identity is durable before any plaintext appears.
        assert!(std::fs::read(&temp).unwrap().is_empty());
        assert!(matches!(
            &load_all(store.path()).unwrap()[0].record,
            RecoveryRecordKind::Artifact(ArtifactRecord {
                state: ArtifactState::Created { .. },
                ..
            })
        ));
        std::fs::write(&temp, "secret-token").unwrap();
        assert!(
            !std::fs::read_to_string(record_path(store.path(), &record_id).unwrap())
                .unwrap()
                .contains("secret-token")
        );
    }
}
