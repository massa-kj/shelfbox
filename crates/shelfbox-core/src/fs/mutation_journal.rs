//! Durable artifact leases for operation-scoped mutations.
//!
//! This module owns D5's temp-path protocol.  Operation workflows select a
//! scope and durable high-level phase, but neither they nor materialization
//! adapters receive an artifact path until this journal has recorded it,
//! installed its exact temporary exclude when needed, created the empty
//! private file, and recorded its stable identity.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use ulid::Ulid;

use crate::{
    domain::{
        copy_safety::{ArtifactScope, PersistentMutation},
        operation_record::{
            ArtifactLocation, ArtifactRecord, ArtifactState, OperationPhase, RecoveryAbsolutePath,
            RecoveryRecord, RecoveryRecordKind, RepoTempExclude, OPERATION_RECORD_SCHEMA_VERSION,
        },
        path::{RepoRelativePath, StoreRelativePath},
    },
    error::{AppError, Result},
    failpoint::{self, Failpoint},
    fs::{
        materializer::{
            ArtifactLease, CommitContext, CommitPermit, DurableOperationPhase, MutationJournal,
            WritableArtifactLease, WritePreconditionGuard,
        },
        permissions::{self, ParentDirMode},
        secure_transfer,
    },
    ignore::IgnoreBackend,
    storage::operation_record_store,
};

/// Mutation journal used by the durable add workflow.
///
/// The store write lock is held by `RepoContext` for this journal's entire
/// lifetime.  The journal intentionally has no manifest responsibilities;
/// high-level operation phases remain owned by the add workflow.
pub(crate) struct AddMutationJournal<'a> {
    store_root: &'a Path,
    repo_root: &'a Path,
    ignore: &'a dyn IgnoreBackend,
    operation: &'a mut RecoveryRecord,
    repo_destination: PathBuf,
    store_destination: PathBuf,
    next_token: u64,
    artifacts: BTreeMap<u64, ActiveArtifact>,
}

struct ActiveArtifact {
    record: RecoveryRecord,
    path: PathBuf,
    native_identity: Option<crate::fs::platform::FileIdentity>,
}

impl<'a> AddMutationJournal<'a> {
    pub(crate) fn new(
        store_root: &'a Path,
        repo_root: &'a Path,
        ignore: &'a dyn IgnoreBackend,
        operation: &'a mut RecoveryRecord,
        repo_destination: PathBuf,
        store_destination: PathBuf,
    ) -> Self {
        Self {
            store_root,
            repo_root,
            ignore,
            operation,
            repo_destination,
            store_destination,
            next_token: 1,
            artifacts: BTreeMap::new(),
        }
    }

    /// Persists a high-level add checkpoint after its physical mutation has
    /// completed. A crash before this call is handled by the Phase 6
    /// current/next-row rule.
    pub(crate) fn advance(&mut self, phase: OperationPhase) -> Result<()> {
        let operation = self.operation_mut()?;
        operation.phase = phase;
        operation_record_store::update(self.store_root, self.operation)
    }

    /// Removes all identity-matched artifact records and their exact temp
    /// excludes after final postcondition validation. A conflict is preserved
    /// instead of deleting a replaced temp.
    pub(crate) fn cleanup_all(&mut self) -> Result<()> {
        let tokens: Vec<u64> = self.artifacts.keys().copied().collect();
        for token in tokens {
            self.cleanup_token(token)?;
        }
        Ok(())
    }

    /// Creates and validates the canonical destination parent after the
    /// operation record and target exclude are durable, but before transfer
    /// planning inspects the missing final component.
    pub(crate) fn ensure_store_destination_parent(&self) -> Result<()> {
        permissions::ensure_parent_dir(&self.store_destination, ParentDirMode::Default)?;
        secure_transfer::validate_parent_path(self.store_root, &self.store_destination)
    }

    fn operation_mut(&mut self) -> Result<&mut crate::domain::operation_record::OperationRecord> {
        match &mut self.operation.record {
            RecoveryRecordKind::Operation(operation) => Ok(operation),
            RecoveryRecordKind::Artifact(_) => Err(AppError::Internal(
                "add mutation journal requires an operation record".into(),
            )),
        }
    }

    fn reserve_artifact(&mut self, scope: ArtifactScope) -> Result<ArtifactLease> {
        let (root, destination) = match scope {
            ArtifactScope::RepoSide => (self.repo_root, &self.repo_destination),
            ArtifactScope::StoreSide => (self.store_root, &self.store_destination),
        };
        permissions::ensure_parent_dir(destination, ParentDirMode::Default)?;
        secure_transfer::validate_parent_path(root, destination)?;
        let parent = destination
            .parent()
            .ok_or_else(|| AppError::UnsafeFilesystemEntry {
                path: destination.clone(),
                reason: "artifact destination has no parent",
            })?;
        let temp_path = secure_transfer::allocate_private_temp_path(
            parent,
            destination.file_name().unwrap_or_default(),
        )?;
        let token = self.next_token;
        self.next_token += 1;
        let record_id = Ulid::new().to_string();
        let repo_id = self.operation_mut()?.repo_id.clone();
        let location = artifact_location(scope, root, &temp_path)?;
        let repo_temp_exclude = match &location {
            ArtifactLocation::Repo { path, .. } => Some(RepoTempExclude {
                path: path.clone(),
                added_by_operation: true,
                verified: false,
            }),
            ArtifactLocation::Store { .. } => None,
        };
        let mut artifact = RecoveryRecord {
            schema_version: OPERATION_RECORD_SCHEMA_VERSION,
            record_id: record_id.clone(),
            created_at: crate::context::now_iso8601(),
            record: RecoveryRecordKind::Artifact(ArtifactRecord {
                repo_id,
                scope,
                location,
                state: ArtifactState::Planned,
                repo_temp_exclude,
            }),
        };
        operation_record_store::create(self.store_root, &artifact)?;

        // The operation record links the independent artifact for inspection
        // and GC protection. The artifact record remains independently
        // recoverable if a crash happens before this update.
        self.operation_mut()?.artifact_record_ids.push(record_id);
        operation_record_store::update(self.store_root, self.operation)?;

        if scope == ArtifactScope::RepoSide {
            let RecoveryRecordKind::Artifact(record) = &mut artifact.record else {
                unreachable!()
            };
            let Some(exclude) = &mut record.repo_temp_exclude else {
                unreachable!()
            };
            self.ignore
                .add_entries(self.repo_root, &[exclude.path.as_str()])?;
            failpoint::after(Failpoint::PersistentMutation(
                PersistentMutation::RepoTempExclude,
            ))?;
            if !self
                .ignore
                .has_entry(self.repo_root, exclude.path.as_str())?
            {
                return Err(AppError::Internal(
                    "managed temporary exclude was not persisted".into(),
                ));
            }
            exclude.verified = true;
            operation_record_store::update(self.store_root, &artifact)?;
        }

        self.artifacts.insert(
            token,
            ActiveArtifact {
                record: artifact,
                path: temp_path,
                native_identity: None,
            },
        );
        Ok(ArtifactLease::from_journal(scope, token))
    }

    fn authorize(&mut self, lease: ArtifactLease) -> Result<WritableArtifactLease> {
        let token = lease.token();
        let active = self
            .artifacts
            .get_mut(&token)
            .ok_or_else(|| AppError::Internal("unknown artifact lease".into()))?;
        if active.native_identity.is_some() {
            return Err(AppError::Internal(
                "artifact lease was already authorized".into(),
            ));
        }
        let native_identity = secure_transfer::create_empty_private_temp_at(&active.path)?;
        failpoint::after(Failpoint::PersistentMutation(
            PersistentMutation::EmptyTempCreation,
        ))?;
        let durable_identity = operation_record_store::identity_from_path(&active.path)?;
        let RecoveryRecordKind::Artifact(artifact) = &mut active.record.record else {
            unreachable!()
        };
        artifact.state = ArtifactState::Created {
            identity: durable_identity,
            plaintext_authorized: true,
        };
        operation_record_store::update(self.store_root, &active.record)?;
        failpoint::after(Failpoint::PersistentMutation(
            PersistentMutation::TempIdentityRecord,
        ))?;
        active.native_identity = Some(native_identity);
        Ok(WritableArtifactLease::from_authorized_temp(
            token,
            active.path.clone(),
            native_identity,
        ))
    }

    fn cleanup_token(&mut self, token: u64) -> Result<()> {
        let Some(active) = self.artifacts.remove(&token) else {
            return Ok(());
        };
        operation_record_store::cleanup_artifact(
            self.store_root,
            self.repo_root,
            &active.record.record_id,
            match &active.record.record {
                RecoveryRecordKind::Artifact(artifact) => artifact,
                RecoveryRecordKind::Operation(_) => unreachable!(),
            },
        )?;
        if let RecoveryRecordKind::Artifact(artifact) = &active.record.record {
            if let Some(exclude) = &artifact.repo_temp_exclude {
                if exclude.added_by_operation && exclude.verified {
                    self.ignore
                        .remove_entries(self.repo_root, &[exclude.path.as_str()])?;
                }
            }
        }
        operation_record_store::remove(self.store_root, &active.record.record_id)?;
        failpoint::after(Failpoint::PersistentMutation(
            PersistentMutation::ArtifactRecordDelete,
        ))?;
        let operation = self.operation_mut()?;
        operation
            .artifact_record_ids
            .retain(|id| id != &active.record.record_id);
        operation_record_store::update(self.store_root, self.operation)
    }
}

impl MutationJournal for AddMutationJournal<'_> {
    fn acquire_artifact_lease(&mut self, scope: ArtifactScope) -> Result<ArtifactLease> {
        self.reserve_artifact(scope)
    }

    fn authorize_plaintext_write(&mut self, lease: ArtifactLease) -> Result<WritableArtifactLease> {
        self.authorize(lease)
    }

    fn record_phase(&mut self, phase: DurableOperationPhase) -> Result<()> {
        let high_level = match phase {
            DurableOperationPhase::CanonicalTransferCommitted => {
                Some(OperationPhase::StoreTransferred)
            }
            DurableOperationPhase::MaterializationCommitted => {
                Some(OperationPhase::RepoMaterialized)
            }
            DurableOperationPhase::PostCommitValidated => Some(OperationPhase::PostCommitValidated),
            DurableOperationPhase::MaterializationPrepared
            | DurableOperationPhase::CanonicalTransferPrepared
            | DurableOperationPhase::CommitAuthorized => None,
        };
        if let Some(phase) = high_level {
            self.advance(phase)?;
        }
        Ok(())
    }

    fn issue_commit_permit(&mut self, guard: WritePreconditionGuard) -> Result<CommitPermit> {
        // The add workflow obtains a fresh materializer/transfer inspection,
        // rechecks Git and excludes, then asks for this permit immediately
        // before commit. Keeping the opaque guard consumed here prevents an
        // operation from carrying a raw identity snapshot across the boundary.
        let _ = guard.required_checks();
        Ok(CommitPermit::issued())
    }

    fn cleanup_prepared_artifact(&mut self, context: CommitContext) -> Result<()> {
        if context.artifact_token() == 0 {
            return Ok(());
        }
        self.cleanup_token(context.artifact_token())
    }
}

fn artifact_location(scope: ArtifactScope, root: &Path, path: &Path) -> Result<ArtifactLocation> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| AppError::UnsafeFilesystemEntry {
            path: path.to_path_buf(),
            reason: "artifact path escapes its trusted root",
        })?;
    let relative = relative.to_string_lossy().replace('\\', "/");
    match scope {
        ArtifactScope::RepoSide => Ok(ArtifactLocation::Repo {
            repo_root: RecoveryAbsolutePath::new(root).ok_or_else(|| {
                AppError::UnsafeFilesystemEntry {
                    path: root.to_path_buf(),
                    reason: "repository root is not a safe absolute path",
                }
            })?,
            path: RepoRelativePath::new(relative).ok_or_else(|| {
                AppError::UnsafeFilesystemEntry {
                    path: path.to_path_buf(),
                    reason: "artifact repo path is not normalized",
                }
            })?,
        }),
        ArtifactScope::StoreSide => Ok(ArtifactLocation::Store {
            path: StoreRelativePath::new(relative).ok_or_else(|| {
                AppError::UnsafeFilesystemEntry {
                    path: path.to_path_buf(),
                    reason: "artifact store path is not normalized",
                }
            })?,
        }),
    }
}
