//! Strategy-neutral canonical store transfer port.
//!
//! Canonical store movement is deliberately separate from repository
//! materialization. Its future adapter may use rename, secure streaming copy,
//! or a cross-device protocol, but operations issue only these typed actions.

use std::fmt;

use crate::{
    domain::{
        copy_safety::ArtifactScope,
        path::{RepoRelativePath, StoreRelativePath},
    },
    error::Result,
    fs::materializer::{
        CommitContext, CommitPermit, DurableOperationPhase, InspectionSnapshot, MutationJournal,
        WritableArtifactLease, WritePreconditionGuard,
    },
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

/// Typed canonical-store actions. They do not select a low-level transfer
/// algorithm and cannot be mistaken for repository materialization actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CanonicalTransferAction {
    Move {
        source: StoreRelativePath,
        destination: StoreRelativePath,
        expected_source: ExpectedCanonicalEntry,
    },
    ReplaceFromRepo {
        source: RepoRelativePath,
        destination: StoreRelativePath,
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
    snapshot: InspectionSnapshot,
}

impl fmt::Debug for CanonicalTransferFacts {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalTransferFacts")
            .field("source_kind", &self.source_kind)
            .field("destination_kind", &self.destination_kind)
            .field("source_hardlink_free", &self.source_hardlink_free)
            .field("destination_hardlink_free", &self.destination_hardlink_free)
            .field("snapshot", &"opaque")
            .finish()
    }
}

impl CanonicalTransferFacts {
    pub(crate) fn expected_source(&self) -> ExpectedCanonicalEntry {
        ExpectedCanonicalEntry {
            kind: self.source_kind,
            snapshot: self.snapshot.clone(),
        }
    }

    pub(crate) fn write_precondition_guard(
        &self,
        context: CommitContext,
    ) -> WritePreconditionGuard {
        WritePreconditionGuard::for_canonical_transfer(self.snapshot.clone(), context)
    }

    #[cfg(test)]
    pub(crate) fn for_test(source_kind: CanonicalEntryKind) -> Self {
        Self {
            source_kind,
            destination_kind: CanonicalEntryKind::Missing,
            source_hardlink_free: true,
            destination_hardlink_free: true,
            snapshot: InspectionSnapshot::for_test(1),
        }
    }
}

/// Prepared canonical transfer with no access to temp paths, identities, or
/// selected transfer algorithm.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PreparedCanonicalTransfer {
    context: CommitContext,
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
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(token: u64) -> Self {
        Self {
            context: CommitContext::for_test(token),
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
        };

        assert!(format!("{action:?}").contains("snapshot: \"opaque\""));
    }
}
