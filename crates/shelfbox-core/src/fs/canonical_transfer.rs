//! Strategy-neutral canonical store transfer port.
//!
//! Canonical store movement is deliberately separate from repository
//! materialization. Its future adapter may use rename, secure streaming copy,
//! or a cross-device protocol, but operations issue only these typed actions.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use crate::{
    domain::{
        copy_safety::ArtifactScope,
        path::{RepoRelativePath, StoreRelativePath},
    },
    error::{AppError, Result},
    failpoint::{self, Failpoint},
    fs::materializer::{
        AuthorizedTemp, CommitContext, CommitPermit, DurableOperationPhase, InspectionSnapshot,
        MutationJournal, WritableArtifactLease, WritePreconditionGuard,
    },
    fs::{platform, secure_transfer},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CanonicalEntryKind {
    Missing,
    RegularFile,
    Directory,
    SymlinkOrReparsePoint,
    Other,
}

/// Opaque expected source/destination state for canonical movement.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ExpectedCanonicalEntry {
    pub kind: CanonicalEntryKind,
    snapshot: InspectionSnapshot,
}

impl fmt::Debug for ExpectedCanonicalEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExpectedCanonicalEntry")
            .field("kind", &self.kind)
            .field("snapshot", &"opaque")
            .finish()
    }
}

impl ExpectedCanonicalEntry {
    /// Placeholder used only while obtaining a planning inspection. The
    /// resulting action must be rebuilt from that inspection's opaque expected
    /// entries before it reaches `prepare` or `commit`.
    pub(crate) fn unchecked(kind: CanonicalEntryKind) -> Self {
        Self {
            kind,
            snapshot: InspectionSnapshot::from_entry(None),
        }
    }
}

/// Typed canonical-store actions. They do not select a low-level transfer
/// algorithm and cannot be mistaken for repository materialization actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CanonicalTransferAction {
    Move {
        source: StoreRelativePath,
        destination: StoreRelativePath,
        expected_source: ExpectedCanonicalEntry,
        /// The destination must still be the inspected missing entry at the
        /// commit boundary.  A store rename is never allowed to replace an
        /// unexpected entry, even when the platform's rename primitive would
        /// permit it.
        expected_destination: ExpectedCanonicalEntry,
    },
    ReplaceFromRepo {
        source: RepoRelativePath,
        destination: StoreRelativePath,
        expected_source: ExpectedCanonicalEntry,
        expected_destination: ExpectedCanonicalEntry,
    },
    /// Copies repo content to the canonical store while preserving the source
    /// materialization. This is intentionally distinct from `ReplaceFromRepo`,
    /// whose add workflow consumes its repo source.
    CopyFromRepo {
        source: RepoRelativePath,
        destination: StoreRelativePath,
        expected_source: ExpectedCanonicalEntry,
        expected_destination: ExpectedCanonicalEntry,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CanonicalInspectionPurpose {
    Planning,
    PreCommit,
    PostCommit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CanonicalTransferInspectionRequest {
    pub action: CanonicalTransferAction,
    pub purpose: CanonicalInspectionPurpose,
}

/// Strategy-neutral no-follow facts for a canonical transfer.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CanonicalTransferFacts {
    pub source_kind: CanonicalEntryKind,
    pub destination_kind: CanonicalEntryKind,
    pub source_hardlink_free: bool,
    pub destination_hardlink_free: bool,
    source_snapshot: InspectionSnapshot,
    destination_snapshot: InspectionSnapshot,
}

impl fmt::Debug for CanonicalTransferFacts {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalTransferFacts")
            .field("source_kind", &self.source_kind)
            .field("destination_kind", &self.destination_kind)
            .field("source_hardlink_free", &self.source_hardlink_free)
            .field("destination_hardlink_free", &self.destination_hardlink_free)
            .field("source_snapshot", &"opaque")
            .field("destination_snapshot", &"opaque")
            .finish()
    }
}

impl CanonicalTransferFacts {
    pub(crate) fn expected_source(&self) -> ExpectedCanonicalEntry {
        ExpectedCanonicalEntry {
            kind: self.source_kind,
            snapshot: self.source_snapshot.clone(),
        }
    }

    pub(crate) fn expected_destination(&self) -> ExpectedCanonicalEntry {
        ExpectedCanonicalEntry {
            kind: self.destination_kind,
            snapshot: self.destination_snapshot.clone(),
        }
    }

    pub(crate) fn write_precondition_guard(
        &self,
        context: CommitContext,
    ) -> WritePreconditionGuard {
        WritePreconditionGuard::for_canonical_transfer(self.source_snapshot.clone(), context)
    }

    #[cfg(test)]
    pub(crate) fn for_test(source_kind: CanonicalEntryKind) -> Self {
        Self {
            source_kind,
            destination_kind: CanonicalEntryKind::Missing,
            source_hardlink_free: true,
            destination_hardlink_free: true,
            source_snapshot: InspectionSnapshot::for_test(1),
            destination_snapshot: InspectionSnapshot::from_entry(None),
        }
    }
}

/// Prepared canonical transfer with no access to temp paths, identities, or
/// selected transfer algorithm.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PreparedCanonicalTransfer {
    context: CommitContext,
    action: Option<CanonicalTransferAction>,
    temp: Option<AuthorizedTemp>,
    direct_rename: bool,
}

impl fmt::Debug for PreparedCanonicalTransfer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PreparedCanonicalTransfer { opaque: true }")
    }
}

impl PreparedCanonicalTransfer {
    pub(crate) fn commit_context(&self) -> CommitContext {
        self.context.clone()
    }

    pub(in crate::fs) fn from_writable_lease(lease: WritableArtifactLease) -> Self {
        Self {
            context: lease.into_commit_context(),
            action: None,
            temp: None,
            direct_rename: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(token: u64) -> Self {
        Self {
            context: CommitContext::for_test(token),
            action: None,
            temp: None,
            direct_rename: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CanonicalTransferCommitOutcome {
    Applied,
}

/// Operation-facing port for canonical store movement.
pub(crate) trait CanonicalTransfer {
    fn inspect(
        &self,
        request: CanonicalTransferInspectionRequest,
    ) -> Result<CanonicalTransferFacts>;

    /// Implementations obtain a store-side artifact lease from `journal` and
    /// must not use a repo-side materialization temp for canonical movement.
    fn prepare(
        &mut self,
        action: CanonicalTransferAction,
        journal: &mut dyn MutationJournal,
    ) -> Result<PreparedCanonicalTransfer>;

    fn commit(
        &mut self,
        prepared: PreparedCanonicalTransfer,
        permit: CommitPermit,
    ) -> Result<CanonicalTransferCommitOutcome>;

    fn abort(
        &mut self,
        prepared: PreparedCanonicalTransfer,
        journal: &mut dyn MutationJournal,
    ) -> Result<()>;
}

/// Concrete canonical transfer adapter used by add. It never selects a repo
/// materialization strategy; it only moves the canonical regular file through
/// a store-side journal artifact when copying is required.
pub(crate) struct DefaultCanonicalTransfer {
    repo_root: PathBuf,
    store_root: PathBuf,
}

impl DefaultCanonicalTransfer {
    pub(crate) fn new(repo_root: PathBuf, store_root: PathBuf) -> Self {
        Self {
            repo_root,
            store_root,
        }
    }

    fn paths(&self, action: &CanonicalTransferAction) -> (PathBuf, PathBuf, &Path, &Path) {
        match action {
            CanonicalTransferAction::Move {
                source,
                destination,
                ..
            } => {
                let source_path = self.store_root.join(source.as_str());
                let destination_path = self.store_root.join(destination.as_str());
                (
                    source_path,
                    destination_path,
                    &self.store_root,
                    &self.store_root,
                )
            }
            CanonicalTransferAction::ReplaceFromRepo {
                source,
                destination,
                ..
            }
            | CanonicalTransferAction::CopyFromRepo {
                source,
                destination,
                ..
            } => {
                let source_path = self.repo_root.join(source.as_str());
                let destination_path = self.store_root.join(destination.as_str());
                (
                    source_path,
                    destination_path,
                    &self.repo_root,
                    &self.store_root,
                )
            }
        }
    }

    fn inspect_entry(path: &Path) -> Result<(CanonicalEntryKind, bool, InspectionSnapshot)> {
        match platform::inspect_no_follow(path) {
            Ok(entry) => Ok((
                match entry.kind {
                    platform::EntryKind::RegularFile => CanonicalEntryKind::RegularFile,
                    platform::EntryKind::Directory => CanonicalEntryKind::Directory,
                    platform::EntryKind::SymlinkOrReparsePoint => {
                        CanonicalEntryKind::SymlinkOrReparsePoint
                    }
                    platform::EntryKind::Other => CanonicalEntryKind::Other,
                },
                entry.link_count <= 1,
                InspectionSnapshot::from_entry(Some(entry)),
            )),
            Err(AppError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                Ok((
                    CanonicalEntryKind::Missing,
                    true,
                    InspectionSnapshot::from_entry(None),
                ))
            }
            Err(error) => Err(error),
        }
    }

    fn ensure_expected(path: &Path, expected: &ExpectedCanonicalEntry) -> Result<()> {
        let actual = match platform::inspect_no_follow(path) {
            Ok(entry) => InspectionSnapshot::from_entry(Some(entry)),
            Err(AppError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                InspectionSnapshot::from_entry(None)
            }
            Err(error) => return Err(error),
        };
        if actual != expected.snapshot {
            return Err(AppError::FilesystemEntryChanged {
                path: path.to_path_buf(),
            });
        }
        Ok(())
    }
}

impl CanonicalTransfer for DefaultCanonicalTransfer {
    fn inspect(
        &self,
        request: CanonicalTransferInspectionRequest,
    ) -> Result<CanonicalTransferFacts> {
        let (source, destination, source_root, destination_root) = self.paths(&request.action);
        secure_transfer::validate_parent_path(source_root, &source)?;
        let (source_kind, source_hardlink_free, source_snapshot) = Self::inspect_entry(&source)?;
        let (destination_kind, destination_hardlink_free, destination_snapshot) =
            match secure_transfer::validate_parent_path(destination_root, &destination) {
                Ok(()) => Self::inspect_entry(&destination)?,
                Err(AppError::Io { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    (
                        CanonicalEntryKind::Missing,
                        true,
                        InspectionSnapshot::from_entry(None),
                    )
                }
                Err(error) => return Err(error),
            };
        Ok(CanonicalTransferFacts {
            source_kind,
            destination_kind,
            source_hardlink_free,
            destination_hardlink_free,
            source_snapshot,
            destination_snapshot,
        })
    }

    fn prepare(
        &mut self,
        action: CanonicalTransferAction,
        journal: &mut dyn MutationJournal,
    ) -> Result<PreparedCanonicalTransfer> {
        match &action {
            CanonicalTransferAction::ReplaceFromRepo {
                expected_source,
                expected_destination,
                ..
            } => {
                let (source, destination, _, _) = self.paths(&action);
                Self::ensure_expected(&source, expected_source)?;
                Self::ensure_expected(&destination, expected_destination)?;
                let destination_parent =
                    destination
                        .parent()
                        .ok_or_else(|| AppError::UnsafeFilesystemEntry {
                            path: destination.clone(),
                            reason: "canonical destination has no parent",
                        })?;
                if platform::inspect_no_follow(&source)?.identity.volume
                    == platform::inspect_no_follow(destination_parent)?
                        .identity
                        .volume
                {
                    return Ok(PreparedCanonicalTransfer {
                        context: CommitContext::for_non_artifact(),
                        action: Some(action),
                        temp: None,
                        direct_rename: true,
                    });
                }
                let lease = journal.acquire_artifact_lease(ArtifactScope::StoreSide)?;
                let writable = journal.authorize_plaintext_write(lease)?;
                let temp = writable.authorized_temp()?.clone();
                secure_transfer::populate_authorized_temp(
                    &self.repo_root,
                    &source,
                    temp.path(),
                    temp.identity(),
                    secure_transfer::PermissionMode::FromSource,
                    None,
                )?;
                let (context, _) = writable.into_parts();
                Ok(PreparedCanonicalTransfer {
                    context,
                    action: Some(action),
                    temp: Some(temp),
                    direct_rename: false,
                })
            }
            CanonicalTransferAction::CopyFromRepo {
                expected_source,
                expected_destination,
                ..
            } => {
                let (source, destination, _, _) = self.paths(&action);
                Self::ensure_expected(&source, expected_source)?;
                Self::ensure_expected(&destination, expected_destination)?;
                if !matches!(
                    expected_destination.kind,
                    CanonicalEntryKind::RegularFile | CanonicalEntryKind::Missing
                ) {
                    return Err(AppError::UnsafeFilesystemEntry {
                        path: destination,
                        reason: "canonical copy destination is not a regular file or missing entry",
                    });
                }
                let lease = journal.acquire_artifact_lease(ArtifactScope::StoreSide)?;
                let writable = journal.authorize_plaintext_write(lease)?;
                let temp = writable.authorized_temp()?.clone();
                let (permission_mode, permission_destination) = match expected_destination.kind {
                    CanonicalEntryKind::RegularFile => (
                        secure_transfer::PermissionMode::PreserveDestination,
                        Some(destination.as_path()),
                    ),
                    CanonicalEntryKind::Missing => {
                        (secure_transfer::PermissionMode::FromSource, None)
                    }
                    _ => unreachable!(),
                };
                secure_transfer::populate_authorized_temp(
                    &self.repo_root,
                    &source,
                    temp.path(),
                    temp.identity(),
                    permission_mode,
                    permission_destination,
                )?;
                let (context, _) = writable.into_parts();
                Ok(PreparedCanonicalTransfer {
                    context,
                    action: Some(action),
                    temp: Some(temp),
                    direct_rename: false,
                })
            }
            CanonicalTransferAction::Move {
                expected_source,
                expected_destination,
                ..
            } => {
                let (source, destination, _, _) = self.paths(&action);
                Self::ensure_expected(&source, expected_source)?;
                Self::ensure_expected(&destination, expected_destination)?;
                if expected_destination.kind != CanonicalEntryKind::Missing {
                    return Err(AppError::FilesystemEntryChanged { path: destination });
                }
                let destination_parent =
                    destination
                        .parent()
                        .ok_or_else(|| AppError::UnsafeFilesystemEntry {
                            path: destination.clone(),
                            reason: "canonical move destination has no parent",
                        })?;
                if platform::inspect_no_follow(&source)?.identity.volume
                    == platform::inspect_no_follow(destination_parent)?
                        .identity
                        .volume
                {
                    return Ok(PreparedCanonicalTransfer {
                        context: CommitContext::for_non_artifact(),
                        action: Some(action),
                        temp: None,
                        direct_rename: true,
                    });
                }

                // Cross-device movement is a copy-sync-install-remove
                // transaction.  The journal owns the private store artifact,
                // so an interruption before source deletion leaves a durable,
                // identity-safe recovery target rather than an untracked
                // plaintext temp.
                let lease = journal.acquire_artifact_lease(ArtifactScope::StoreSide)?;
                let writable = journal.authorize_plaintext_write(lease)?;
                let temp = writable.authorized_temp()?.clone();
                secure_transfer::populate_authorized_temp(
                    &self.store_root,
                    &source,
                    temp.path(),
                    temp.identity(),
                    secure_transfer::PermissionMode::FromSource,
                    None,
                )?;
                let (context, _) = writable.into_parts();
                Ok(PreparedCanonicalTransfer {
                    context,
                    action: Some(action),
                    temp: Some(temp),
                    direct_rename: false,
                })
            }
        }
    }

    fn commit(
        &mut self,
        prepared: PreparedCanonicalTransfer,
        permit: CommitPermit,
    ) -> Result<CanonicalTransferCommitOutcome> {
        permit.require_context(&prepared.context)?;
        let action = prepared
            .action
            .ok_or_else(|| AppError::Internal("missing prepared canonical action".into()))?;
        match action {
            CanonicalTransferAction::ReplaceFromRepo {
                source,
                destination,
                expected_source,
                expected_destination,
                ..
            } => {
                let source = self.repo_root.join(source.as_str());
                let destination = self.store_root.join(destination.as_str());
                Self::ensure_expected(&source, &expected_source)?;
                Self::ensure_expected(&destination, &expected_destination)?;
                if prepared.direct_rename {
                    std::fs::rename(&source, &destination)
                        .map_err(|error| AppError::io(&source, error))?;
                } else {
                    let temp = prepared.temp.ok_or_else(|| {
                        AppError::Internal("missing prepared canonical temp".into())
                    })?;
                    secure_transfer::commit_authorized_temp(
                        &self.store_root,
                        temp.path(),
                        temp.identity(),
                        &destination,
                    )?;
                    Self::ensure_expected(&source, &expected_source)?;
                    std::fs::remove_file(&source).map_err(|error| AppError::io(&source, error))?;
                }
                failpoint::after(Failpoint::PersistentMutation(
                    crate::domain::copy_safety::PersistentMutation::DestinationReplacement,
                ))?;
                Ok(CanonicalTransferCommitOutcome::Applied)
            }
            CanonicalTransferAction::CopyFromRepo {
                source,
                destination,
                expected_source,
                expected_destination,
            } => {
                let source = self.repo_root.join(source.as_str());
                let destination = self.store_root.join(destination.as_str());
                Self::ensure_expected(&source, &expected_source)?;
                Self::ensure_expected(&destination, &expected_destination)?;
                let temp = prepared.temp.ok_or_else(|| {
                    AppError::Internal("missing prepared canonical sync temp".into())
                })?;
                secure_transfer::commit_authorized_temp(
                    &self.store_root,
                    temp.path(),
                    temp.identity(),
                    &destination,
                )?;
                // Recheck the source after replacement: it must remain the
                // durable repo-side observation recorded for this direction.
                Self::ensure_expected(&source, &expected_source)?;
                failpoint::after(Failpoint::PersistentMutation(
                    crate::domain::copy_safety::PersistentMutation::DestinationReplacement,
                ))?;
                Ok(CanonicalTransferCommitOutcome::Applied)
            }
            CanonicalTransferAction::Move {
                source,
                destination,
                expected_source,
                expected_destination,
            } => {
                let source = self.store_root.join(source.as_str());
                let destination = self.store_root.join(destination.as_str());
                Self::ensure_expected(&source, &expected_source)?;
                Self::ensure_expected(&destination, &expected_destination)?;
                if prepared.direct_rename {
                    std::fs::rename(&source, &destination)
                        .map_err(|error| AppError::io(&source, error))?;
                } else {
                    let temp = prepared.temp.ok_or_else(|| {
                        AppError::Internal("missing prepared canonical move temp".into())
                    })?;
                    secure_transfer::commit_authorized_temp(
                        &self.store_root,
                        temp.path(),
                        temp.identity(),
                        &destination,
                    )?;
                    // The old canonical identity is still required before
                    // removal; a same-path replacement race must preserve
                    // both copies and leave the durable record for recovery.
                    Self::ensure_expected(&source, &expected_source)?;
                    std::fs::remove_file(&source).map_err(|error| AppError::io(&source, error))?;
                }
                failpoint::after(Failpoint::PersistentMutation(
                    crate::domain::copy_safety::PersistentMutation::DestinationReplacement,
                ))?;
                Ok(CanonicalTransferCommitOutcome::Applied)
            }
        }
    }

    fn abort(
        &mut self,
        prepared: PreparedCanonicalTransfer,
        journal: &mut dyn MutationJournal,
    ) -> Result<()> {
        journal.cleanup_prepared_artifact(prepared.context)
    }
}

/// Documents the opaque artifact protocol expected from canonical adapters.
/// Future implementations call these in order through a `MutationJournal`.
pub(crate) fn prepare_store_side_artifact(
    journal: &mut dyn MutationJournal,
) -> Result<WritableArtifactLease> {
    let lease = journal.acquire_artifact_lease(ArtifactScope::StoreSide)?;
    journal.authorize_plaintext_write(lease)
}

/// Shared high-level checkpoints used by an operation orchestrating a
/// canonical transfer. Keeping this list here prevents a transfer adapter from
/// becoming a hidden operation coordinator.
pub(crate) const CANONICAL_TRANSFER_PHASES: &[DurableOperationPhase] = &[
    DurableOperationPhase::CanonicalTransferPrepared,
    DurableOperationPhase::CommitAuthorized,
    DurableOperationPhase::CanonicalTransferCommitted,
    DurableOperationPhase::PostCommitValidated,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_action_keeps_its_identity_snapshot_opaque() {
        let facts = CanonicalTransferFacts::for_test(CanonicalEntryKind::RegularFile);
        let action = CanonicalTransferAction::Move {
            source: "items/old.env".parse().unwrap(),
            destination: "items/new.env".parse().unwrap(),
            expected_source: facts.expected_source(),
            expected_destination: facts.expected_destination(),
        };

        assert!(format!("{action:?}").contains("snapshot: \"opaque\""));
    }

    #[test]
    fn commit_rejects_a_permit_for_a_different_prepared_transfer() {
        let repo = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let mut transfer =
            DefaultCanonicalTransfer::new(repo.path().to_path_buf(), store.path().to_path_buf());

        let error = transfer
            .commit(
                PreparedCanonicalTransfer::for_test(41),
                CommitPermit::for_test(42),
            )
            .unwrap_err();
        assert!(
            matches!(error, AppError::Internal(message) if message.contains("does not authorize"))
        );
    }
}
