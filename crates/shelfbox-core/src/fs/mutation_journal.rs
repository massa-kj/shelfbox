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
    git,
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

    /// Records the identity and fingerprint of a long-lived operation backup
    /// after its transfer completed.  Unlike ordinary artifacts, this backup
    /// intentionally survives until manifest/exclude finalization, but it is
    /// still deleted only through identity-safe recovery cleanup.
    pub(crate) fn record_backup_from_path(
        &mut self,
        path: &Path,
    ) -> Result<crate::domain::operation_record::RecoveryBackupMetadata> {
        let identity = operation_record_store::identity_from_path(path)?;
        let fingerprint =
            crate::domain::recovery_fingerprint::RecoveryFingerprint::from_file(path)?;
        let operation = self.operation_mut()?;
        let backup = operation.backup.as_mut().ok_or_else(|| {
            AppError::Internal("operation has no recovery backup to record".into())
        })?;
        backup.expected_identity = Some(identity);
        backup.fingerprint = Some(fingerprint);
        let recorded = backup.clone();
        operation_record_store::update(self.store_root, self.operation)?;
        Ok(recorded)
    }

    fn operation_mut(&mut self) -> Result<&mut crate::domain::operation_record::OperationRecord> {
        match &mut self.operation.record {
            RecoveryRecordKind::Operation(operation) => Ok(operation),
            RecoveryRecordKind::Artifact(_) => Err(AppError::Internal(
                "add mutation journal requires an operation record".into(),
            )),
        }
    }

    fn operation(&self) -> Result<&crate::domain::operation_record::OperationRecord> {
        match &self.operation.record {
            RecoveryRecordKind::Operation(operation) => Ok(operation),
            RecoveryRecordKind::Artifact(_) => Err(AppError::Internal(
                "add mutation journal requires an operation record".into(),
            )),
        }
    }

    /// Revalidates every condition that this journal can observe immediately
    /// before issuing a permit.  The transfer/materializer commit then checks
    /// the opaque identity snapshots bound to that permit.  Git and exclude
    /// state are deliberately checked again here because neither is part of
    /// the filesystem atomic-replace primitive.
    fn validate_commit_preconditions(&self, guard: &WritePreconditionGuard) -> Result<()> {
        let operation = self.operation()?;
        permissions::ensure_parent_dir(&self.repo_destination, ParentDirMode::Default)?;
        secure_transfer::validate_parent_path(self.repo_root, &self.repo_destination)?;
        secure_transfer::validate_parent_path(self.store_root, &self.store_destination)?;

        if git::is_tracked(self.repo_root, &self.repo_destination)? {
            return Err(AppError::PathIsTracked {
                path: self.repo_destination.clone(),
            });
        }
        let repo_path = operation.pre_state.repo_path.as_ref().ok_or_else(|| {
            AppError::Internal("add operation is missing its repository path pre-state".into())
        })?;
        if !self.ignore.has_entry(self.repo_root, repo_path.as_str())? {
            return Err(AppError::Internal(
                "managed add exclude was removed before commit authorization".into(),
            ));
        }

        for active in self.artifacts.values() {
            let RecoveryRecordKind::Artifact(artifact) = &active.record.record else {
                unreachable!()
            };
            if let Some(exclude) = &artifact.repo_temp_exclude {
                if exclude.added_by_operation
                    && exclude.verified
                    && !self
                        .ignore
                        .has_entry(self.repo_root, exclude.path.as_str())?
                {
                    return Err(AppError::Internal(
                        "managed temporary exclude was removed before commit authorization".into(),
                    ));
                }
            }
        }

        let token = guard.commit_context().artifact_token();
        if token != 0 {
            let active = self.artifacts.get(&token).ok_or_else(|| {
                AppError::Internal("commit guard references an unknown artifact lease".into())
            })?;
            let RecoveryRecordKind::Artifact(artifact) = &active.record.record else {
                unreachable!()
            };
            if !matches!(
                artifact.state,
                ArtifactState::Created {
                    plaintext_authorized: true,
                    ..
                }
            ) || active.native_identity.is_none()
            {
                return Err(AppError::Internal(
                    "commit guard references an artifact without durable plaintext authorization"
                        .into(),
                ));
            }
        }
        Ok(())
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
            durability: self.operation.durability,
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
        let native_identity =
            secure_transfer::create_empty_private_temp_at(&active.path, active.record.durability)?;
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
        // The operation supplies fresh no-follow facts, while the journal
        // performs the Git/exclude and durable-artifact half of D5 again at
        // the authorization boundary.  The resulting permit is accepted only
        // by the prepared mutation whose opaque context it carries.
        let _ = guard.required_checks();
        self.validate_commit_preconditions(&guard)?;
        Ok(CommitPermit::from_guard(guard))
    }

    fn cleanup_prepared_artifact(&mut self, context: CommitContext) -> Result<()> {
        if context.artifact_token() == 0 {
            return Ok(());
        }
        self.cleanup_token(context.artifact_token())
    }
}

/// Durable operation journal for explicit content synchronization.
///
/// Sync shares the D5 artifact protocol and Git/exclude authorization checks
/// with add, but its enclosing operation record has a different recovery
/// truth table.  This small wrapper deliberately exposes only the checkpoints
/// that make sense for a one-direction content replacement.
pub(crate) struct SyncMutationJournal<'a> {
    inner: AddMutationJournal<'a>,
}

impl<'a> SyncMutationJournal<'a> {
    pub(crate) fn new(
        store_root: &'a Path,
        repo_root: &'a Path,
        ignore: &'a dyn IgnoreBackend,
        operation: &'a mut RecoveryRecord,
        repo_destination: PathBuf,
        store_destination: PathBuf,
    ) -> Self {
        Self {
            inner: AddMutationJournal::new(
                store_root,
                repo_root,
                ignore,
                operation,
                repo_destination,
                store_destination,
            ),
        }
    }

    pub(crate) fn advance_content_synchronized(&mut self) -> Result<()> {
        self.inner.advance(OperationPhase::ContentSynchronized)
    }

    pub(crate) fn advance_post_commit_validated(&mut self) -> Result<()> {
        self.inner.advance(OperationPhase::PostCommitValidated)
    }

    pub(crate) fn cleanup_all(&mut self) -> Result<()> {
        self.inner.cleanup_all()
    }
}

impl MutationJournal for SyncMutationJournal<'_> {
    fn acquire_artifact_lease(&mut self, scope: ArtifactScope) -> Result<ArtifactLease> {
        self.inner.acquire_artifact_lease(scope)
    }

    fn authorize_plaintext_write(&mut self, lease: ArtifactLease) -> Result<WritableArtifactLease> {
        self.inner.authorize_plaintext_write(lease)
    }

    fn record_phase(&mut self, phase: DurableOperationPhase) -> Result<()> {
        self.inner.record_phase(phase)
    }

    fn issue_commit_permit(&mut self, guard: WritePreconditionGuard) -> Result<CommitPermit> {
        self.inner.issue_commit_permit(guard)
    }

    fn cleanup_prepared_artifact(&mut self, context: CommitContext) -> Result<()> {
        self.inner.cleanup_prepared_artifact(context)
    }
}

/// Artifact-only journal for repair operations.
///
/// Repair deliberately has no high-level ownership transaction: it either
/// recreates a missing materialization or leaves existing user content alone.
/// A regular-copy repair still needs the complete D5 temp protocol, so this
/// journal persists independent recovery-artifact records without inventing a
/// misleading add/move operation record.
pub(crate) struct RepairMutationJournal<'a> {
    store_root: &'a Path,
    repo_root: &'a Path,
    ignore: &'a dyn IgnoreBackend,
    repo_id: String,
    target_exclude: String,
    durability: crate::domain::mutation_durability::MutationDurability,
    repo_destination: PathBuf,
    store_destination: PathBuf,
    next_token: u64,
    artifacts: BTreeMap<u64, ActiveArtifact>,
}

impl<'a> RepairMutationJournal<'a> {
    pub(crate) fn new(
        store_root: &'a Path,
        repo_root: &'a Path,
        ignore: &'a dyn IgnoreBackend,
        repo_id: impl Into<String>,
        target_exclude: impl Into<String>,
        repo_destination: PathBuf,
        store_destination: PathBuf,
    ) -> Self {
        Self {
            store_root,
            repo_root,
            ignore,
            repo_id: repo_id.into(),
            target_exclude: target_exclude.into(),
            durability: crate::domain::mutation_durability::MutationDurability::Require,
            repo_destination,
            store_destination,
            next_token: 1,
            artifacts: BTreeMap::new(),
        }
    }

    /// Binds the command-resolved durability policy once for this journal.
    /// The default is strict for direct/internal construction; mutation entry
    /// points must explicitly supply their resolved local setting.
    pub(crate) fn with_durability(
        mut self,
        durability: crate::domain::mutation_durability::MutationDurability,
    ) -> Self {
        self.durability = durability;
        self
    }

    /// Removes only identity-matching temps after the caller has validated
    /// the final materialization.  A failure leaves the durable artifact
    /// record for the normal recovery gate instead of guessing at cleanup.
    pub(crate) fn cleanup_all(&mut self) -> Result<()> {
        let tokens: Vec<u64> = self.artifacts.keys().copied().collect();
        for token in tokens {
            self.cleanup_token(token)?;
        }
        Ok(())
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
        let location = artifact_location(scope, root, &temp_path)?;
        let repo_temp_exclude = match &location {
            ArtifactLocation::Repo { path, .. } => Some(RepoTempExclude {
                path: path.clone(),
                added_by_operation: true,
                verified: false,
            }),
            ArtifactLocation::Store { .. } => None,
        };
        let mut record = RecoveryRecord {
            schema_version: OPERATION_RECORD_SCHEMA_VERSION,
            durability: self.durability,
            record_id: Ulid::new().to_string(),
            created_at: crate::context::now_iso8601(),
            record: RecoveryRecordKind::Artifact(ArtifactRecord {
                repo_id: self.repo_id.clone(),
                scope,
                location,
                state: ArtifactState::Planned,
                repo_temp_exclude,
            }),
        };
        operation_record_store::create(self.store_root, &record)?;

        if scope == ArtifactScope::RepoSide {
            let RecoveryRecordKind::Artifact(artifact) = &mut record.record else {
                unreachable!()
            };
            let Some(exclude) = &mut artifact.repo_temp_exclude else {
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
            operation_record_store::update(self.store_root, &record)?;
        }

        self.artifacts.insert(
            token,
            ActiveArtifact {
                record,
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
        let native_identity =
            secure_transfer::create_empty_private_temp_at(&active.path, active.record.durability)?;
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

    fn validate_commit_preconditions(&self, guard: &WritePreconditionGuard) -> Result<()> {
        permissions::ensure_parent_dir(&self.repo_destination, ParentDirMode::Default)?;
        secure_transfer::validate_parent_path(self.repo_root, &self.repo_destination)?;
        secure_transfer::validate_parent_path(self.store_root, &self.store_destination)?;
        if git::is_tracked(self.repo_root, &self.repo_destination)? {
            return Err(AppError::PathIsTracked {
                path: self.repo_destination.clone(),
            });
        }
        if !self
            .ignore
            .has_entry(self.repo_root, self.target_exclude.as_str())?
        {
            return Err(AppError::Internal(
                "managed repair exclude was removed before commit authorization".into(),
            ));
        }
        for active in self.artifacts.values() {
            let RecoveryRecordKind::Artifact(artifact) = &active.record.record else {
                unreachable!()
            };
            if let Some(exclude) = &artifact.repo_temp_exclude {
                if exclude.added_by_operation
                    && exclude.verified
                    && !self
                        .ignore
                        .has_entry(self.repo_root, exclude.path.as_str())?
                {
                    return Err(AppError::Internal(
                        "managed temporary exclude was removed before commit authorization".into(),
                    ));
                }
            }
        }
        let token = guard.commit_context().artifact_token();
        if token != 0 {
            let active = self.artifacts.get(&token).ok_or_else(|| {
                AppError::Internal("commit guard references an unknown artifact lease".into())
            })?;
            let RecoveryRecordKind::Artifact(artifact) = &active.record.record else {
                unreachable!()
            };
            if !matches!(
                artifact.state,
                ArtifactState::Created {
                    plaintext_authorized: true,
                    ..
                }
            ) || active.native_identity.is_none()
            {
                return Err(AppError::Internal(
                    "commit guard references an artifact without durable plaintext authorization"
                        .into(),
                ));
            }
        }
        Ok(())
    }

    fn cleanup_token(&mut self, token: u64) -> Result<()> {
        let Some(active) = self.artifacts.remove(&token) else {
            return Ok(());
        };
        let RecoveryRecordKind::Artifact(artifact) = &active.record.record else {
            unreachable!()
        };
        operation_record_store::cleanup_artifact(
            self.store_root,
            self.repo_root,
            &active.record.record_id,
            artifact,
        )?;
        if let Some(exclude) = &artifact.repo_temp_exclude {
            if exclude.added_by_operation && exclude.verified {
                self.ignore
                    .remove_entries(self.repo_root, &[exclude.path.as_str()])?;
            }
        }
        operation_record_store::remove(self.store_root, &active.record.record_id)?;
        failpoint::after(Failpoint::PersistentMutation(
            PersistentMutation::ArtifactRecordDelete,
        ))
    }
}

impl MutationJournal for RepairMutationJournal<'_> {
    fn acquire_artifact_lease(&mut self, scope: ArtifactScope) -> Result<ArtifactLease> {
        self.reserve_artifact(scope)
    }

    fn authorize_plaintext_write(&mut self, lease: ArtifactLease) -> Result<WritableArtifactLease> {
        self.authorize(lease)
    }

    fn record_phase(&mut self, _phase: DurableOperationPhase) -> Result<()> {
        Ok(())
    }

    fn issue_commit_permit(&mut self, guard: WritePreconditionGuard) -> Result<CommitPermit> {
        let _ = guard.required_checks();
        self.validate_commit_preconditions(&guard)?;
        Ok(CommitPermit::from_guard(guard))
    }

    fn cleanup_prepared_artifact(&mut self, context: CommitContext) -> Result<()> {
        if context.artifact_token() == 0 {
            return Ok(());
        }
        self.cleanup_token(context.artifact_token())
    }
}

fn artifact_location(scope: ArtifactScope, root: &Path, path: &Path) -> Result<ArtifactLocation> {
    let relative = secure_transfer::relative_path_in_trusted_root(root, path)?;
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

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::*;

    #[cfg(unix)]
    #[test]
    fn artifact_location_accepts_uncreated_temp_via_trusted_root_alias() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("repo");
        let alias_parent = tempfile::tempdir().unwrap();
        let alias = alias_parent.path().join("alias");
        std::fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(parent.path(), &alias).unwrap();

        let temp = alias.join("repo/.secret.temporary");
        let location = artifact_location(ArtifactScope::RepoSide, &root, &temp).unwrap();

        match location {
            ArtifactLocation::Repo { repo_root, path } => {
                assert_eq!(repo_root.as_path(), root);
                assert_eq!(path.as_str(), ".secret.temporary");
            }
            ArtifactLocation::Store { .. } => panic!("expected a repository artifact location"),
        }
    }
}
