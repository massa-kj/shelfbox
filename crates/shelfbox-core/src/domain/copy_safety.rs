//! Copy mutation crash-safety contract.
//!
//! D5 freezes ordering and responsibility before copy mutation code exists.
//! The constants in this module are not an implementation of recovery or
//! materialization; they are the contract that Phase 2, Phase 5, and Phase 6
//! code must satisfy.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ArtifactScope {
    RepoSide,
    StoreSide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CopyMutationStep {
    GenerateTempPathWithoutCreating,
    DurablyRecordArtifactPath,
    AddAndVerifyExactRepoTempExclude,
    CreateEmptyPrivateTempCreateNew,
    DurablyRecordTempIdentity,
    AuthorizePlaintextWrite,
    RevalidateBeforeCommit,
    AtomicallyReplaceDestination,
    RevalidateAfterMaterialization,
    CleanupArtifactAfterFinalVerification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum WritePreconditionCheck {
    RepoAndStoreRootContainment,
    ParentNoFollow,
    FinalComponentNoFollow,
    FileIdentity,
    LinkCountAndHardlinkAbsence,
    GitTrackedState,
    TargetExclude,
    ArtifactLeaseExcludes,
    DestinationMatchesPlannedState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PersistentMutation {
    ArtifactPathRecord,
    RepoTempExclude,
    EmptyTempCreation,
    TempIdentityRecord,
    PlaintextWrite,
    DestinationReplacement,
    PostMaterializationValidationRecord,
    ArtifactRecordDelete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum BoundaryOwner {
    MutationJournal,
    MaterializerOrCanonicalTransfer,
    Operation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum BoundaryResponsibility {
    AllocateTempPath,
    DurablyRecordArtifactPath,
    EnsureRepoTempExactExclude,
    CreateEmptyPrivateTemp,
    DurablyRecordTempIdentity,
    AuthorizeWritableTemp,
    PopulatePreparedArtifact,
    AtomicallyCommitPreparedArtifact,
    AdvanceHighLevelDurablePhase,
    ObtainWritePreconditionGuard,
    ValidatePostMaterializationState,
    DecideRecoveryDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum StaleCompletionRule {
    ClassifyAsCompletedWhenFinalPostconditionsHold,
    RemoveOnlyStaleRecordAndMatchingArtifacts,
    NeverRollbackCompletedUserVisibleState,
    RecordArtifactAndBackupPathsBeforeCreation,
}

pub(crate) const REPO_SIDE_TEMP_PROTOCOL: &[CopyMutationStep] = &[
    CopyMutationStep::GenerateTempPathWithoutCreating,
    CopyMutationStep::DurablyRecordArtifactPath,
    CopyMutationStep::AddAndVerifyExactRepoTempExclude,
    CopyMutationStep::CreateEmptyPrivateTempCreateNew,
    CopyMutationStep::DurablyRecordTempIdentity,
    CopyMutationStep::AuthorizePlaintextWrite,
    CopyMutationStep::RevalidateBeforeCommit,
    CopyMutationStep::AtomicallyReplaceDestination,
    CopyMutationStep::RevalidateAfterMaterialization,
    CopyMutationStep::CleanupArtifactAfterFinalVerification,
];

pub(crate) const STORE_SIDE_TEMP_PROTOCOL: &[CopyMutationStep] = &[
    CopyMutationStep::GenerateTempPathWithoutCreating,
    CopyMutationStep::DurablyRecordArtifactPath,
    CopyMutationStep::CreateEmptyPrivateTempCreateNew,
    CopyMutationStep::DurablyRecordTempIdentity,
    CopyMutationStep::AuthorizePlaintextWrite,
    CopyMutationStep::RevalidateBeforeCommit,
    CopyMutationStep::AtomicallyReplaceDestination,
    CopyMutationStep::RevalidateAfterMaterialization,
    CopyMutationStep::CleanupArtifactAfterFinalVerification,
];

pub(crate) const WRITE_PRECONDITION_CHECKS: &[WritePreconditionCheck] = &[
    WritePreconditionCheck::RepoAndStoreRootContainment,
    WritePreconditionCheck::ParentNoFollow,
    WritePreconditionCheck::FinalComponentNoFollow,
    WritePreconditionCheck::FileIdentity,
    WritePreconditionCheck::LinkCountAndHardlinkAbsence,
    WritePreconditionCheck::GitTrackedState,
    WritePreconditionCheck::TargetExclude,
    WritePreconditionCheck::ArtifactLeaseExcludes,
    WritePreconditionCheck::DestinationMatchesPlannedState,
];

pub(crate) const FAILPOINT_REQUIRED_AFTER: &[PersistentMutation] = &[
    PersistentMutation::ArtifactPathRecord,
    PersistentMutation::RepoTempExclude,
    PersistentMutation::EmptyTempCreation,
    PersistentMutation::TempIdentityRecord,
    PersistentMutation::PlaintextWrite,
    PersistentMutation::DestinationReplacement,
    PersistentMutation::PostMaterializationValidationRecord,
    PersistentMutation::ArtifactRecordDelete,
];

pub(crate) const STALE_COMPLETION_RULES: &[StaleCompletionRule] = &[
    StaleCompletionRule::ClassifyAsCompletedWhenFinalPostconditionsHold,
    StaleCompletionRule::RemoveOnlyStaleRecordAndMatchingArtifacts,
    StaleCompletionRule::NeverRollbackCompletedUserVisibleState,
    StaleCompletionRule::RecordArtifactAndBackupPathsBeforeCreation,
];

pub(crate) const BOUNDARY_RESPONSIBILITIES: &[(BoundaryOwner, BoundaryResponsibility)] = &[
    (
        BoundaryOwner::MutationJournal,
        BoundaryResponsibility::AllocateTempPath,
    ),
    (
        BoundaryOwner::MutationJournal,
        BoundaryResponsibility::DurablyRecordArtifactPath,
    ),
    (
        BoundaryOwner::MutationJournal,
        BoundaryResponsibility::EnsureRepoTempExactExclude,
    ),
    (
        BoundaryOwner::MutationJournal,
        BoundaryResponsibility::CreateEmptyPrivateTemp,
    ),
    (
        BoundaryOwner::MutationJournal,
        BoundaryResponsibility::DurablyRecordTempIdentity,
    ),
    (
        BoundaryOwner::MutationJournal,
        BoundaryResponsibility::AuthorizeWritableTemp,
    ),
    (
        BoundaryOwner::MaterializerOrCanonicalTransfer,
        BoundaryResponsibility::PopulatePreparedArtifact,
    ),
    (
        BoundaryOwner::MaterializerOrCanonicalTransfer,
        BoundaryResponsibility::AtomicallyCommitPreparedArtifact,
    ),
    (
        BoundaryOwner::Operation,
        BoundaryResponsibility::AdvanceHighLevelDurablePhase,
    ),
    (
        BoundaryOwner::Operation,
        BoundaryResponsibility::ObtainWritePreconditionGuard,
    ),
    (
        BoundaryOwner::Operation,
        BoundaryResponsibility::ValidatePostMaterializationState,
    ),
    (
        BoundaryOwner::Operation,
        BoundaryResponsibility::DecideRecoveryDirection,
    ),
];

pub(crate) fn protocol_for(scope: ArtifactScope) -> &'static [CopyMutationStep] {
    match scope {
        ArtifactScope::RepoSide => REPO_SIDE_TEMP_PROTOCOL,
        ArtifactScope::StoreSide => STORE_SIDE_TEMP_PROTOCOL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_side_temp_protocol_records_and_excludes_before_creation() {
        let protocol = protocol_for(ArtifactScope::RepoSide);

        assert_before(
            protocol,
            CopyMutationStep::GenerateTempPathWithoutCreating,
            CopyMutationStep::DurablyRecordArtifactPath,
        );
        assert_before(
            protocol,
            CopyMutationStep::DurablyRecordArtifactPath,
            CopyMutationStep::AddAndVerifyExactRepoTempExclude,
        );
        assert_before(
            protocol,
            CopyMutationStep::AddAndVerifyExactRepoTempExclude,
            CopyMutationStep::CreateEmptyPrivateTempCreateNew,
        );
        assert_before(
            protocol,
            CopyMutationStep::CreateEmptyPrivateTempCreateNew,
            CopyMutationStep::DurablyRecordTempIdentity,
        );
        assert_before(
            protocol,
            CopyMutationStep::DurablyRecordTempIdentity,
            CopyMutationStep::AuthorizePlaintextWrite,
        );
    }

    #[test]
    fn store_side_temp_protocol_skips_exclude_but_keeps_write_ahead_identity_order() {
        let protocol = protocol_for(ArtifactScope::StoreSide);

        assert!(
            !protocol.contains(&CopyMutationStep::AddAndVerifyExactRepoTempExclude),
            "store-side temps do not need Git excludes"
        );
        assert_before(
            protocol,
            CopyMutationStep::DurablyRecordArtifactPath,
            CopyMutationStep::CreateEmptyPrivateTempCreateNew,
        );
        assert_before(
            protocol,
            CopyMutationStep::DurablyRecordTempIdentity,
            CopyMutationStep::AuthorizePlaintextWrite,
        );
    }

    #[test]
    fn commit_validation_brackets_atomic_replacement_and_cleanup() {
        for scope in [ArtifactScope::RepoSide, ArtifactScope::StoreSide] {
            let protocol = protocol_for(scope);

            assert_before(
                protocol,
                CopyMutationStep::RevalidateBeforeCommit,
                CopyMutationStep::AtomicallyReplaceDestination,
            );
            assert_before(
                protocol,
                CopyMutationStep::AtomicallyReplaceDestination,
                CopyMutationStep::RevalidateAfterMaterialization,
            );
            assert_before(
                protocol,
                CopyMutationStep::RevalidateAfterMaterialization,
                CopyMutationStep::CleanupArtifactAfterFinalVerification,
            );
        }
    }

    #[test]
    fn write_precondition_guard_has_the_full_revalidation_checklist() {
        assert_eq!(
            WRITE_PRECONDITION_CHECKS,
            &[
                WritePreconditionCheck::RepoAndStoreRootContainment,
                WritePreconditionCheck::ParentNoFollow,
                WritePreconditionCheck::FinalComponentNoFollow,
                WritePreconditionCheck::FileIdentity,
                WritePreconditionCheck::LinkCountAndHardlinkAbsence,
                WritePreconditionCheck::GitTrackedState,
                WritePreconditionCheck::TargetExclude,
                WritePreconditionCheck::ArtifactLeaseExcludes,
                WritePreconditionCheck::DestinationMatchesPlannedState,
            ]
        );
    }

    #[test]
    fn failpoint_proof_is_required_after_every_persistent_mutation() {
        assert_eq!(
            FAILPOINT_REQUIRED_AFTER,
            &[
                PersistentMutation::ArtifactPathRecord,
                PersistentMutation::RepoTempExclude,
                PersistentMutation::EmptyTempCreation,
                PersistentMutation::TempIdentityRecord,
                PersistentMutation::PlaintextWrite,
                PersistentMutation::DestinationReplacement,
                PersistentMutation::PostMaterializationValidationRecord,
                PersistentMutation::ArtifactRecordDelete,
            ]
        );
    }

    #[test]
    fn boundary_responsibilities_keep_temp_details_opaque_to_operations() {
        assert_owner(
            BoundaryResponsibility::AllocateTempPath,
            BoundaryOwner::MutationJournal,
        );
        assert_owner(
            BoundaryResponsibility::DurablyRecordArtifactPath,
            BoundaryOwner::MutationJournal,
        );
        assert_owner(
            BoundaryResponsibility::DurablyRecordTempIdentity,
            BoundaryOwner::MutationJournal,
        );
        assert_owner(
            BoundaryResponsibility::PopulatePreparedArtifact,
            BoundaryOwner::MaterializerOrCanonicalTransfer,
        );
        assert_owner(
            BoundaryResponsibility::AtomicallyCommitPreparedArtifact,
            BoundaryOwner::MaterializerOrCanonicalTransfer,
        );
        assert_owner(
            BoundaryResponsibility::AdvanceHighLevelDurablePhase,
            BoundaryOwner::Operation,
        );
        assert_owner(
            BoundaryResponsibility::ObtainWritePreconditionGuard,
            BoundaryOwner::Operation,
        );
        assert_owner(
            BoundaryResponsibility::DecideRecoveryDirection,
            BoundaryOwner::Operation,
        );
    }

    #[test]
    fn stale_completion_recovery_never_rolls_back_completed_state() {
        assert_eq!(
            STALE_COMPLETION_RULES,
            &[
                StaleCompletionRule::ClassifyAsCompletedWhenFinalPostconditionsHold,
                StaleCompletionRule::RemoveOnlyStaleRecordAndMatchingArtifacts,
                StaleCompletionRule::NeverRollbackCompletedUserVisibleState,
                StaleCompletionRule::RecordArtifactAndBackupPathsBeforeCreation,
            ]
        );
    }

    fn assert_before(
        protocol: &[CopyMutationStep],
        before: CopyMutationStep,
        after: CopyMutationStep,
    ) {
        let before_index = protocol
            .iter()
            .position(|step| *step == before)
            .unwrap_or_else(|| panic!("missing protocol step: {before:?}"));
        let after_index = protocol
            .iter()
            .position(|step| *step == after)
            .unwrap_or_else(|| panic!("missing protocol step: {after:?}"));

        assert!(
            before_index < after_index,
            "expected {before:?} before {after:?} in {protocol:?}"
        );
    }

    fn assert_owner(responsibility: BoundaryResponsibility, owner: BoundaryOwner) {
        assert!(
            BOUNDARY_RESPONSIBILITIES.contains(&(owner, responsibility)),
            "expected {owner:?} to own {responsibility:?}"
        );
    }
}
