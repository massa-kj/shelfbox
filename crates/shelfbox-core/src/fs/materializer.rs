//! Operation-facing materialization port.
//!
//! The concrete adapter composes `LinkStrategy`, secure transfer, and platform
//! capabilities. The operation layer may use the types and traits here, but
//! cannot inspect the opaque handles or snapshots that carry filesystem details.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use crate::{
    domain::{
        copy_safety::{ArtifactScope, WritePreconditionCheck, WRITE_PRECONDITION_CHECKS},
        materialization::{CopyContentState, MaterializationStrategy},
        path::{RepoRelativePath, StoreRelativePath},
    },
    error::{AppError, Result},
    failpoint::{self, Failpoint},
    fs::{platform, secure_transfer},
    link::{DefaultLinkStrategy, LinkStrategy},
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
    pub store_exists: bool,
    pub copy_content: CopyContentState,
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
            .field("store_exists", &self.store_exists)
            .field("copy_content", &self.copy_content)
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
            store_exists: true,
            copy_content: CopyContentState::NotCompared,
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
    pub(in crate::fs) fn from_journal(scope: ArtifactScope, token: u64) -> Self {
        Self {
            scope,
            reference: ArtifactLeaseReference(token),
        }
    }

    pub(in crate::fs) const fn token(&self) -> u64 {
        self.reference.0
    }

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
    authorized_temp: Option<AuthorizedTemp>,
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

    pub(in crate::fs) fn from_authorized_temp(
        token: u64,
        path: PathBuf,
        identity: platform::FileIdentity,
    ) -> Self {
        Self {
            reference: ArtifactLeaseReference(token),
            authorized_temp: Some(AuthorizedTemp { path, identity }),
        }
    }

    pub(in crate::fs) fn authorized_temp(&self) -> Result<&AuthorizedTemp> {
        self.authorized_temp
            .as_ref()
            .ok_or_else(|| AppError::Internal("missing authorized artifact temp".into()))
    }

    pub(in crate::fs) fn into_parts(self) -> (CommitContext, Option<AuthorizedTemp>) {
        (
            CommitContext {
                artifact_lease: self.reference,
            },
            self.authorized_temp,
        )
    }

    #[cfg(test)]
    pub(crate) fn for_test(token: u64) -> Self {
        Self {
            reference: ArtifactLeaseReference(token),
            authorized_temp: None,
        }
    }
}

/// Filesystem-adapter-private details of a writable artifact. Operations only
/// receive its opaque [`WritableArtifactLease`].
#[derive(Clone, PartialEq, Eq)]
pub(in crate::fs) struct AuthorizedTemp {
    path: PathBuf,
    identity: platform::FileIdentity,
}

impl AuthorizedTemp {
    pub(in crate::fs) fn path(&self) -> &Path {
        &self.path
    }

    pub(in crate::fs) const fn identity(&self) -> platform::FileIdentity {
        self.identity
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
    pub(in crate::fs) fn for_non_artifact() -> Self {
        Self {
            artifact_lease: ArtifactLeaseReference(0),
        }
    }

    pub(in crate::fs) const fn artifact_token(&self) -> u64 {
        self.artifact_lease.0
    }

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
    action: PreparedAction,
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
            action: PreparedAction::NoOp,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(token: u64) -> Self {
        Self {
            context: CommitContext::for_test(token),
            action: PreparedAction::NoOp,
        }
    }
}

/// Adapter-private execution state.  It is deliberately not exposed through
/// `PreparedMaterialization`, which keeps temporary paths and dispatch details
/// out of operation code.
#[derive(Clone, PartialEq, Eq)]
enum PreparedAction {
    NoOp,
    Execute(MaterializationAction),
    CopyCreate {
        location: MaterializationLocation,
        temp: AuthorizedTemp,
    },
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
    pub(crate) fn issued() -> Self {
        Self { token: 0 }
    }

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

/// Journal used only by recovery actions that cannot create plaintext temps.
/// It permits symlink-only forward recovery while making any request for a
/// repo/store temporary artifact fail closed.
pub(crate) struct NoArtifactJournal;

impl MutationJournal for NoArtifactJournal {
    fn acquire_artifact_lease(&mut self, _scope: ArtifactScope) -> Result<ArtifactLease> {
        Err(AppError::Internal(
            "recovery attempted to create a plaintext artifact without a durable journal".into(),
        ))
    }

    fn authorize_plaintext_write(
        &mut self,
        _lease: ArtifactLease,
    ) -> Result<WritableArtifactLease> {
        Err(AppError::Internal(
            "recovery attempted to authorize plaintext without a durable journal".into(),
        ))
    }

    fn record_phase(&mut self, _phase: DurableOperationPhase) -> Result<()> {
        Ok(())
    }

    fn issue_commit_permit(&mut self, _guard: WritePreconditionGuard) -> Result<CommitPermit> {
        Ok(CommitPermit::issued())
    }

    fn cleanup_prepared_artifact(&mut self, _context: CommitContext) -> Result<()> {
        Ok(())
    }
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

/// Default repository materializer, constructed only by the composition root.
///
/// It is intentionally generic over the link adapter so focused tests can use
/// a deterministic fake.  Production callers use [`DefaultLinkStrategy`].
pub(crate) struct DefaultMaterializer<L = DefaultLinkStrategy> {
    repo_root: PathBuf,
    store_root: PathBuf,
    link: L,
}

impl DefaultMaterializer<DefaultLinkStrategy> {
    pub(crate) fn new(repo_root: PathBuf, store_root: PathBuf) -> Self {
        Self::with_link_strategy(repo_root, store_root, DefaultLinkStrategy)
    }
}

impl<L> DefaultMaterializer<L> {
    pub(crate) fn with_link_strategy(repo_root: PathBuf, store_root: PathBuf, link: L) -> Self {
        Self {
            repo_root,
            store_root,
            link,
        }
    }

    fn paths(&self, location: &MaterializationLocation) -> (PathBuf, PathBuf) {
        (
            self.repo_root.join(location.repo_path.as_str()),
            self.store_root.join(location.store_path.as_str()),
        )
    }

    fn validate_location(&self, location: &MaterializationLocation) -> Result<(PathBuf, PathBuf)> {
        let (repo, store) = self.paths(location);
        secure_transfer::validate_parent_path(&self.repo_root, &repo)?;
        secure_transfer::validate_parent_path(&self.store_root, &store)?;
        Ok((repo, store))
    }
}

impl<L: LinkStrategy> DefaultMaterializer<L> {
    fn inspect_location(&self, location: &MaterializationLocation) -> Result<MaterializationFacts> {
        let (repo, store) = self.validate_location(location)?;
        let store_exists = !matches!(
            platform::inspect_no_follow(&store),
            Err(AppError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound
        );
        let entry = match platform::inspect_no_follow(&repo) {
            Ok(entry) => entry,
            Err(AppError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(MaterializationFacts {
                    repo_entry_kind: RepoEntryKind::Missing,
                    final_component: FinalComponentInspection::Missing,
                    link_count: None,
                    hardlink_free: true,
                    store_exists,
                    copy_content: CopyContentState::NotCompared,
                    snapshot: InspectionSnapshot::from_entry(None),
                });
            }
            Err(error) => return Err(error),
        };

        let repo_entry_kind = match entry.kind {
            platform::EntryKind::RegularFile => RepoEntryKind::RegularFile,
            platform::EntryKind::Directory => RepoEntryKind::Directory,
            platform::EntryKind::Other => RepoEntryKind::Other,
            platform::EntryKind::SymlinkOrReparsePoint => {
                if self.is_expected_managed_link(&repo, &store) {
                    RepoEntryKind::ManagedSymlink
                } else {
                    RepoEntryKind::UnmanagedSymlinkOrReparsePoint
                }
            }
        };
        let copy_content = if repo_entry_kind == RepoEntryKind::RegularFile && store_exists {
            match secure_transfer::compare_regular_files(&repo, &store) {
                Ok(true) => CopyContentState::Equal,
                Ok(false) => CopyContentState::Diverged,
                Err(_) => CopyContentState::ComparisonFailed,
            }
        } else {
            CopyContentState::NotCompared
        };
        Ok(MaterializationFacts {
            repo_entry_kind,
            final_component: FinalComponentInspection::InspectedWithoutFollowing,
            link_count: Some(entry.link_count),
            hardlink_free: entry.link_count <= 1,
            store_exists,
            copy_content,
            snapshot: InspectionSnapshot::from_entry(Some(entry)),
        })
    }

    fn is_expected_managed_link(&self, link_path: &Path, expected_store: &Path) -> bool {
        if !self.link.is_link(link_path) {
            return false;
        }
        let Ok(target) = self.link.read_target(link_path) else {
            return false;
        };
        let target = if target.is_absolute() {
            target
        } else {
            link_path.parent().unwrap_or(&self.repo_root).join(target)
        };
        // Both paths were containment-checked before this method is called.
        // Canonicalization makes a relative managed link compare equal without
        // treating any other store entry as the expected item.
        target.canonicalize().ok() == expected_store.canonicalize().ok()
    }

    fn execute(&self, action: MaterializationAction) -> Result<MaterializationCommitOutcome> {
        match action {
            MaterializationAction::NoOp => Ok(MaterializationCommitOutcome::NoOp),
            MaterializationAction::Create { location, strategy } => {
                let (repo, store) = self.validate_location(&location)?;
                self.ensure_missing(&repo)?;
                self.create(&store, &repo, strategy)?;
                Ok(MaterializationCommitOutcome::Applied)
            }
            MaterializationAction::Replace {
                location,
                strategy,
                expected,
            } => {
                let (repo, store) = self.validate_location(&location)?;
                self.ensure_expected(&repo, &expected)?;
                self.replace(&store, &repo, strategy)?;
                Ok(MaterializationCommitOutcome::Applied)
            }
            MaterializationAction::Remove { location, expected } => {
                let (repo, _) = self.validate_location(&location)?;
                self.ensure_expected(&repo, &expected)?;
                std::fs::remove_file(&repo).map_err(|e| AppError::io(&repo, e))?;
                Ok(MaterializationCommitOutcome::Applied)
            }
            MaterializationAction::RestoreToRegular { location, expected } => {
                let (repo, store) = self.validate_location(&location)?;
                self.ensure_expected(&repo, &expected)?;
                secure_transfer::copy_replace(
                    &self.store_root,
                    &store,
                    &self.repo_root,
                    &repo,
                    secure_transfer::PermissionMode::FromSource,
                )?;
                Ok(MaterializationCommitOutcome::Applied)
            }
        }
    }

    fn ensure_missing(&self, path: &Path) -> Result<()> {
        match platform::inspect_no_follow(path) {
            Err(AppError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                Ok(())
            }
            Ok(_) => Err(AppError::UnsafeFilesystemEntry {
                path: path.to_path_buf(),
                reason: "destination is no longer missing",
            }),
            Err(error) => Err(error),
        }
    }

    fn ensure_expected(&self, path: &Path, expected: &ExpectedMaterialization) -> Result<()> {
        if InspectionSnapshot::from_entry(Some(platform::inspect_no_follow(path)?))
            != expected.snapshot
        {
            return Err(AppError::FilesystemEntryChanged {
                path: path.to_path_buf(),
            });
        }
        Ok(())
    }

    fn create(&self, store: &Path, repo: &Path, strategy: MaterializationStrategy) -> Result<()> {
        match strategy {
            MaterializationStrategy::Symlink => self.link.create(store, repo),
            MaterializationStrategy::Copy => secure_transfer::copy_replace(
                &self.store_root,
                store,
                &self.repo_root,
                repo,
                secure_transfer::PermissionMode::FromSource,
            ),
        }
    }

    fn replace(&self, store: &Path, repo: &Path, strategy: MaterializationStrategy) -> Result<()> {
        // Copy replacement is already atomic.  A symlink is prepared beside
        // its destination and atomically installed through the platform port.
        match strategy {
            MaterializationStrategy::Copy => secure_transfer::copy_replace(
                &self.store_root,
                store,
                &self.repo_root,
                repo,
                secure_transfer::PermissionMode::FromSource,
            ),
            MaterializationStrategy::Symlink => {
                let parent = repo
                    .parent()
                    .ok_or_else(|| AppError::UnsafeFilesystemEntry {
                        path: repo.to_path_buf(),
                        reason: "destination has no parent",
                    })?;
                let temp = parent.join(format!(".shelfbox-link-{}.tmp", ulid::Ulid::new()));
                self.link.create(store, &temp)?;
                platform::atomic_replace(&temp, repo)
            }
        }
    }
}

impl<L: LinkStrategy> Materializer for DefaultMaterializer<L> {
    fn inspect(&self, request: MaterializationInspectionRequest) -> Result<MaterializationFacts> {
        self.inspect_location(&request.location)
    }

    fn prepare(
        &mut self,
        action: MaterializationAction,
        journal: &mut dyn MutationJournal,
    ) -> Result<PreparedMaterialization> {
        match action {
            MaterializationAction::Create {
                location,
                strategy: MaterializationStrategy::Copy,
            } => {
                let (repo, store) = self.validate_location(&location)?;
                self.ensure_missing(&repo)?;
                let lease = journal.acquire_artifact_lease(ArtifactScope::RepoSide)?;
                let writable = journal.authorize_plaintext_write(lease)?;
                let temp = writable.authorized_temp()?.clone();
                secure_transfer::populate_authorized_temp(
                    &self.store_root,
                    &store,
                    temp.path(),
                    temp.identity(),
                    secure_transfer::PermissionMode::FromSource,
                )?;
                let (context, _) = writable.into_parts();
                Ok(PreparedMaterialization {
                    context,
                    action: PreparedAction::CopyCreate { location, temp },
                })
            }
            action => Ok(PreparedMaterialization {
                // Symlink creation contains no plaintext temporary artifact.
                context: CommitContext::for_non_artifact(),
                action: PreparedAction::Execute(action),
            }),
        }
    }

    fn commit(
        &mut self,
        prepared: PreparedMaterialization,
        _permit: CommitPermit,
    ) -> Result<MaterializationCommitOutcome> {
        let outcome = match prepared.action {
            PreparedAction::NoOp => Ok(MaterializationCommitOutcome::NoOp),
            PreparedAction::Execute(action) => self.execute(action),
            PreparedAction::CopyCreate { location, temp } => {
                let (repo, _) = self.validate_location(&location)?;
                self.ensure_missing(&repo)?;
                secure_transfer::commit_authorized_temp(
                    &self.repo_root,
                    temp.path(),
                    temp.identity(),
                    &repo,
                )?;
                Ok(MaterializationCommitOutcome::Applied)
            }
        }?;
        if outcome == MaterializationCommitOutcome::Applied {
            failpoint::after(Failpoint::PersistentMutation(
                crate::domain::copy_safety::PersistentMutation::DestinationReplacement,
            ))?;
        }
        Ok(outcome)
    }

    fn abort(
        &mut self,
        prepared: PreparedMaterialization,
        journal: &mut dyn MutationJournal,
    ) -> Result<()> {
        journal.cleanup_prepared_artifact(prepared.context)
    }
}

/// An opaque no-follow identity snapshot. The concrete Phase 3 materializer
/// will populate it from the D1 platform adapter. Keeping the representation
/// private lets that implementation evolve without changing operation APIs.
#[derive(Clone, PartialEq, Eq)]
pub(in crate::fs) struct InspectionSnapshot(Option<platform::InspectedEntry>);

impl InspectionSnapshot {
    #[cfg(test)]
    pub(in crate::fs) fn for_test(token: u64) -> Self {
        Self(Some(platform::InspectedEntry {
            kind: platform::EntryKind::RegularFile,
            identity: platform::FileIdentity {
                volume: token,
                file: [0; 16],
            },
            link_count: 1,
        }))
    }
}

impl InspectionSnapshot {
    pub(in crate::fs) fn from_entry(entry: Option<platform::InspectedEntry>) -> Self {
        Self(entry)
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

    #[test]
    fn default_materializer_inspects_regular_copy_and_missing_entry_without_writes() {
        let repo = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        std::fs::create_dir(store.path().join("items")).unwrap();
        let repo_file = repo.path().join("secret.env");
        let store_file = store.path().join("items/secret.env");
        std::fs::write(&repo_file, "repo copy").unwrap();
        std::fs::write(&store_file, "repo copy").unwrap();
        let materializer =
            DefaultMaterializer::new(repo.path().to_path_buf(), store.path().to_path_buf());
        let regular = materializer
            .inspect(MaterializationInspectionRequest {
                location: MaterializationLocation::new(
                    "secret.env".parse().unwrap(),
                    "items/secret.env".parse().unwrap(),
                ),
                purpose: InspectionPurpose::Planning,
            })
            .unwrap();
        assert_eq!(regular.repo_entry_kind, RepoEntryKind::RegularFile);
        assert!(regular.hardlink_free);
        assert!(regular.store_exists);
        assert_eq!(regular.copy_content, CopyContentState::Equal);
        assert_eq!(std::fs::read_to_string(&repo_file).unwrap(), "repo copy");

        std::fs::write(&repo_file, "diverged copy").unwrap();
        let diverged = materializer
            .inspect(MaterializationInspectionRequest {
                location: MaterializationLocation::new(
                    "secret.env".parse().unwrap(),
                    "items/secret.env".parse().unwrap(),
                ),
                purpose: InspectionPurpose::Planning,
            })
            .unwrap();
        assert_eq!(diverged.copy_content, CopyContentState::Diverged);

        let missing = materializer
            .inspect(MaterializationInspectionRequest {
                location: MaterializationLocation::new(
                    "missing.env".parse().unwrap(),
                    "items/missing.env".parse().unwrap(),
                ),
                purpose: InspectionPurpose::Planning,
            })
            .unwrap();
        assert_eq!(missing.repo_entry_kind, RepoEntryKind::Missing);
        assert_eq!(missing.final_component, FinalComponentInspection::Missing);
    }

    #[test]
    #[cfg(unix)]
    fn default_materializer_distinguishes_expected_and_wrong_target_symlinks() {
        let repo = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let item = store.path().join("items/secret.env");
        std::fs::create_dir_all(item.parent().unwrap()).unwrap();
        std::fs::write(&item, "secret").unwrap();
        let path = repo.path().join("secret.env");
        DefaultLinkStrategy.create(&item, &path).unwrap();
        let materializer =
            DefaultMaterializer::new(repo.path().to_path_buf(), store.path().to_path_buf());
        let request = |store_path: &str| MaterializationInspectionRequest {
            location: MaterializationLocation::new(
                "secret.env".parse().unwrap(),
                store_path.parse().unwrap(),
            ),
            purpose: InspectionPurpose::Planning,
        };
        assert_eq!(
            materializer
                .inspect(request("items/secret.env"))
                .unwrap()
                .repo_entry_kind,
            RepoEntryKind::ManagedSymlink
        );
        assert_eq!(
            materializer
                .inspect(request("items/other.env"))
                .unwrap()
                .repo_entry_kind,
            RepoEntryKind::UnmanagedSymlinkOrReparsePoint
        );
    }
}
