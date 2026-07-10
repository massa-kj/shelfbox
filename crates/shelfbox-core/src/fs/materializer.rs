//! Operation-facing materialization port.
//!
//! This module freezes the D6 contract; it intentionally contains no default
//! filesystem adapter. Phase 3 will implement the port inside `fs` by
//! composing `LinkStrategy`, secure transfer, and platform capabilities. The
//! operation layer may use the types and traits here, but cannot inspect the
//! opaque handles or snapshots that carry filesystem details.

use std::fmt;

use crate::{
    domain::{
        copy_safety::{ArtifactScope, WritePreconditionCheck, WRITE_PRECONDITION_CHECKS},
        materialization::MaterializationStrategy,
        path::{RepoRelativePath, StoreRelativePath},
    },
    error::Result,
};

/// Logical endpoints of a repo materialization.
///
/// These are normalized relative paths. The materializer, which is configured
/// with repository and store roots in the composition root, resolves them to
/// absolute paths internally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MaterializationLocation {
    pub repo_path: RepoRelativePath,
    pub store_path: StoreRelativePath,
}

impl MaterializationLocation {
    pub(crate) fn new(repo_path: RepoRelativePath, store_path: StoreRelativePath) -> Self {
        Self {
            repo_path,
            store_path,
        }
    }
}

/// High-level kind observed at a repo materialization path.
///
/// This is deliberately distinct from the platform adapter's entry kind. It
/// contains only policy-relevant facts and never exposes a raw file identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepoEntryKind {
    Missing,
    RegularFile,
    ManagedSymlink,
    UnmanagedSymlinkOrReparsePoint,
    Directory,
    Other,
}

/// Result of inspecting the final component without following it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinalComponentInspection {
    Missing,
    InspectedWithoutFollowing,
}

/// Opaque expected state for a destructive materialization action.
///
/// Operations can see the expected entry kind for policy/reporting, but only
/// the materializer can read the identity snapshot used to prove it still
/// refers to the same filesystem entry at commit time.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ExpectedMaterialization {
    pub repo_entry_kind: RepoEntryKind,
    snapshot: InspectionSnapshot,
}

impl fmt::Debug for ExpectedMaterialization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExpectedMaterialization")
            .field("repo_entry_kind", &self.repo_entry_kind)
            .field("snapshot", &"opaque")
            .finish()
    }
}

/// Read-only facts returned by [`Materializer::inspect`].
///
/// `link_count` and `hardlink_free` are derived from no-follow handle facts.
/// The backing identity is deliberately not exposed; call [`Self::expected`]
/// to carry it forward into a typed mutation action.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct MaterializationFacts {
    pub repo_entry_kind: RepoEntryKind,
    pub final_component: FinalComponentInspection,
    pub link_count: Option<u64>,
    pub hardlink_free: bool,
    snapshot: InspectionSnapshot,
}

impl fmt::Debug for MaterializationFacts {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MaterializationFacts")
            .field("repo_entry_kind", &self.repo_entry_kind)
            .field("final_component", &self.final_component)
            .field("link_count", &self.link_count)
            .field("hardlink_free", &self.hardlink_free)
            .field("snapshot", &"opaque")
            .finish()
    }
}

impl MaterializationFacts {
    pub(crate) fn expected(&self) -> ExpectedMaterialization {
        ExpectedMaterialization {
            repo_entry_kind: self.repo_entry_kind,
            snapshot: self.snapshot.clone(),
        }
    }

    /// Builds a guard from a fresh no-follow materializer inspection.
    ///
    /// The operation performs its Git/exclude policy checks before requesting
    /// the permit; the guard carries the materializer's opaque identity and
    /// no-follow snapshot into the journal's commit authorization step.
    pub(crate) fn write_precondition_guard(
        &self,
        context: CommitContext,
    ) -> WritePreconditionGuard {
        WritePreconditionGuard::from_materialization_facts(self.snapshot.clone(), context)
    }

    #[cfg(test)]
    pub(crate) fn for_test(repo_entry_kind: RepoEntryKind) -> Self {
        Self {
            repo_entry_kind,
            final_component: FinalComponentInspection::InspectedWithoutFollowing,
            link_count: Some(1),
            hardlink_free: true,
            snapshot: InspectionSnapshot::for_test(1),
        }
    }
}

/// A policy-approved filesystem action. Rejection, warning, confirmation, and
/// user intent remain operation/policy decisions rather than action variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MaterializationAction {
    NoOp,
    Create {
        location: MaterializationLocation,
        strategy: MaterializationStrategy,
    },
    Replace {
        location: MaterializationLocation,
        strategy: MaterializationStrategy,
        expected: ExpectedMaterialization,
    },
    Remove {
        location: MaterializationLocation,
        expected: ExpectedMaterialization,
    },
    RestoreToRegular {
        location: MaterializationLocation,
        expected: ExpectedMaterialization,
    },
}

impl MaterializationAction {
    pub(crate) fn location(&self) -> Option<&MaterializationLocation> {
        match self {
            Self::NoOp => None,
            Self::Create { location, .. }
            | Self::Replace { location, .. }
            | Self::Remove { location, .. }
            | Self::RestoreToRegular { location, .. } => Some(location),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InspectionPurpose {
    Planning,
    PreCommit,
    PostCommit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MaterializationInspectionRequest {
    pub location: MaterializationLocation,
    pub purpose: InspectionPurpose,
}

/// High-level durable checkpoints an operation may persist around a commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DurableOperationPhase {
    MaterializationPrepared,
    CanonicalTransferPrepared,
    CommitAuthorized,
    MaterializationCommitted,
    CanonicalTransferCommitted,
    PostCommitValidated,
}

/// Journal-owned reservation for an artifact that may eventually hold
/// plaintext. It has no path or identity accessors.
#[derive(PartialEq, Eq)]
pub(crate) struct ArtifactLease {
    scope: ArtifactScope,
    reference: ArtifactLeaseReference,
}

impl fmt::Debug for ArtifactLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtifactLease")
            .field("scope", &self.scope)
            .field("reference", &"opaque")
            .finish()
    }
}

impl ArtifactLease {
    #[cfg(test)]
    pub(crate) fn for_test(scope: ArtifactScope, token: u64) -> Self {
        Self {
            scope,
            reference: ArtifactLeaseReference(token),
        }
    }
}

/// A lease after the journal has completed the D5 write-ahead barriers and
/// authorized a plaintext write. It remains opaque to operations.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct WritableArtifactLease {
    reference: ArtifactLeaseReference,
}

impl fmt::Debug for WritableArtifactLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WritableArtifactLease { opaque: true }")
    }
}

impl WritableArtifactLease {
    pub(in crate::fs) fn into_commit_context(self) -> CommitContext {
        CommitContext {
            artifact_lease: self.reference,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(token: u64) -> Self {
        Self {
            reference: ArtifactLeaseReference(token),
        }
    }
}

/// Opaque lease reference retained by a prepared mutation for precondition
/// and cleanup coordination.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ArtifactLeaseReference(u64);

impl fmt::Debug for ArtifactLeaseReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ArtifactLeaseReference { opaque: true }")
    }
}

/// Context tied to the prepared artifact. It may be passed to the guard, but
/// offers no temp path, file identity, or lease identifier accessor.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CommitContext {
    artifact_lease: ArtifactLeaseReference,
}

impl fmt::Debug for CommitContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CommitContext { opaque: true }")
    }
}

impl CommitContext {
    #[cfg(test)]
    pub(crate) fn for_test(token: u64) -> Self {
        Self {
            artifact_lease: ArtifactLeaseReference(token),
        }
    }
}

/// Prepared repo materialization. Operations can advance a durable phase and
/// request a permit, but cannot discover how a symlink or copy was prepared.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PreparedMaterialization {
    context: CommitContext,
}

impl fmt::Debug for PreparedMaterialization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PreparedMaterialization { opaque: true }")
    }
}

impl PreparedMaterialization {
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

/// Opaque authorization issued only after fresh write preconditions pass.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CommitPermit {
    token: u64,
}

impl fmt::Debug for CommitPermit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CommitPermit { opaque: true }")
    }
}

impl CommitPermit {
    #[cfg(test)]
    pub(crate) fn for_test(token: u64) -> Self {
        Self { token }
    }
}

/// A fresh commit-time guard. It holds opaque materializer facts and artifact
/// lease context; the operations layer cannot substitute a raw platform fact.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct WritePreconditionGuard {
    materialization_snapshot: InspectionSnapshot,
    context: CommitContext,
}

impl fmt::Debug for WritePreconditionGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WritePreconditionGuard { opaque_facts: true }")
    }
}

impl WritePreconditionGuard {
    fn from_materialization_facts(
        materialization_snapshot: InspectionSnapshot,
        context: CommitContext,
    ) -> Self {
        Self {
            materialization_snapshot,
            context,
        }
    }

    /// The fixed D5 checklist that the operation's precondition implementation
    /// must evaluate immediately before a destructive commit.
    pub(crate) const fn required_checks(&self) -> &'static [WritePreconditionCheck] {
        WRITE_PRECONDITION_CHECKS
    }

    pub(in crate::fs) fn for_canonical_transfer(
        snapshot: InspectionSnapshot,
        context: CommitContext,
    ) -> Self {
        Self {
            materialization_snapshot: snapshot,
            context,
        }
    }
}

/// Result of committing a prepared materialization without leaking transfer
/// implementation details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MaterializationCommitOutcome {
    Applied,
    NoOp,
}

/// Operation-facing mutation journal contract.
///
/// `acquire_artifact_lease` performs the D5 path-recording, repo-side exclude,
/// private create-new, and identity-recording barriers. Only
/// `authorize_plaintext_write` may return a writable lease. Concrete adapters
/// use that lease internally; operations never receive a temp path or handle.
pub(crate) trait MutationJournal {
    fn acquire_artifact_lease(&mut self, scope: ArtifactScope) -> Result<ArtifactLease>;

    fn authorize_plaintext_write(&mut self, lease: ArtifactLease) -> Result<WritableArtifactLease>;

    fn record_phase(&mut self, phase: DurableOperationPhase) -> Result<()>;

    fn issue_commit_permit(&mut self, guard: WritePreconditionGuard) -> Result<CommitPermit>;

    fn cleanup_prepared_artifact(&mut self, context: CommitContext) -> Result<()>;
}

/// Operation-facing repository materialization port.
///
/// Implementations own symlink/copy dispatch, secure transfer, no-follow
/// handling, and artifact population. Operations own durable phase updates,
/// Git/exclude policy checks, confirmation, and recovery direction.
pub(crate) trait Materializer {
    fn inspect(&self, request: MaterializationInspectionRequest) -> Result<MaterializationFacts>;

    fn prepare(
        &mut self,
        action: MaterializationAction,
        journal: &mut dyn MutationJournal,
    ) -> Result<PreparedMaterialization>;

    fn commit(
        &mut self,
        prepared: PreparedMaterialization,
        permit: CommitPermit,
    ) -> Result<MaterializationCommitOutcome>;

    fn abort(
        &mut self,
        prepared: PreparedMaterialization,
        journal: &mut dyn MutationJournal,
    ) -> Result<()>;
}

/// An opaque no-follow identity snapshot. The concrete Phase 3 materializer
/// will populate it from the D1 platform adapter. Keeping the representation
/// private lets that implementation evolve without changing operation APIs.
#[derive(Clone, PartialEq, Eq)]
pub(in crate::fs) struct InspectionSnapshot(u64);

impl InspectionSnapshot {
    #[cfg(test)]
    pub(in crate::fs) fn for_test(token: u64) -> Self {
        Self(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_actions_keep_identity_snapshots_opaque() {
        let facts = MaterializationFacts::for_test(RepoEntryKind::ManagedSymlink);
        let expected = facts.expected();
        let target = MaterializationLocation::new(
            "secret.env".parse().unwrap(),
            "items/secret.env".parse().unwrap(),
        );
        let action = MaterializationAction::Replace {
            location: target,
            strategy: MaterializationStrategy::Copy,
            expected,
        };

        let rendered = format!("{action:?}");
        assert!(rendered.contains("snapshot: \"opaque\""));
        assert!(!rendered.contains("file_identity"));
    }

    #[test]
    fn materializer_facts_produce_the_full_precommit_checklist() {
        let facts = MaterializationFacts::for_test(RepoEntryKind::RegularFile);
        let guard = facts.write_precondition_guard(CommitContext::for_test(7));

        assert_eq!(guard.required_checks(), WRITE_PRECONDITION_CHECKS);
        assert_eq!(
            facts.final_component,
            FinalComponentInspection::InspectedWithoutFollowing
        );
        assert!(facts.hardlink_free);
    }
}
