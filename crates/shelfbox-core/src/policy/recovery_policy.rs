//! Typed, side-effect-free recovery truth tables.
//!
//! The operation layer collects the facts in this module using no-follow
//! inspection, fingerprints, manifest state, and exact exclude ownership.
//! This module deliberately receives only the resulting semantic facts.  It
//! has no filesystem, Git, or storage dependencies, so every automatic
//! recovery direction is reviewable as a row in a table below.
//!
//! A fact value other than an explicitly listed row is a conflict.  In
//! particular, callers must classify a changed identity, fingerprint,
//! manifest membership, or exclude ownership as `Unexpected`, never as a
//! near match.

use crate::domain::operation_record::{OperationDirection, OperationKind, OperationPhase};

/// Input to the recovery truth-table evaluator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RecoveryPolicyInput {
    pub operation: OperationKind,
    pub phase: OperationPhase,
    /// Direction is required only for durable directional relink.
    pub direction: Option<OperationDirection>,
    pub facts: RecoveryFacts,
}

/// Semantic physical facts collected for one durable operation.
///
/// Each operation has a distinct type so it is impossible for a caller to
/// accidentally use an add row to classify a move or a restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryFacts {
    Add(AddRecoveryFacts),
    Restore(RestoreRecoveryFacts),
    Move(MoveRecoveryFacts),
    DirectionalRelink(DirectionalRelinkRecoveryFacts),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AddRecoveryFacts {
    pub repo: AddRepoFact,
    pub store: AddStoreFact,
    pub manifest: AddManifestFact,
    pub exclude: AddExcludeFact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AddRepoFact {
    Source,
    Missing,
    Materialized,
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AddStoreFact {
    Missing,
    Source,
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AddManifestFact {
    Absent,
    Present,
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AddExcludeFact {
    BeforeState,
    Present,
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RestoreRecoveryFacts {
    pub repo: RestoreRepoFact,
    pub store: RestoreStoreFact,
    pub manifest: RestoreManifestFact,
    pub exclude: RestoreExcludeFact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestoreRepoFact {
    OriginalMaterialization,
    RegularMatchingCanonical,
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestoreStoreFact {
    CanonicalPresent,
    CanonicalMissingBackupPresent,
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestoreManifestFact {
    Present,
    Absent,
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestoreExcludeFact {
    BeforeState,
    FinalState,
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MoveRecoveryFacts {
    pub repo: MoveRepoFact,
    pub store: MoveStoreFact,
    pub manifest: MoveManifestFact,
    pub exclude: MoveExcludeFact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoveRepoFact {
    OldMaterializedNewMissing,
    OldMissingNewMaterialized,
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoveStoreFact {
    OldPresentNewMissing,
    OldMissingNewPresent,
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoveManifestFact {
    OldPath,
    NewPath,
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoveExcludeFact {
    BeforeState,
    OldAndNew,
    NewOnly,
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirectionalRelinkRecoveryFacts {
    pub content: DirectionalRelinkContentFact,
    pub manifest: DirectionalRelinkManifestFact,
    pub exclude: DirectionalRelinkExcludeFact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectionalRelinkContentFact {
    BeforeDirectionBackupAbsent,
    SynchronizedBackupPresent,
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectionalRelinkManifestFact {
    Detached,
    Attached,
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectionalRelinkExcludeFact {
    BeforeState,
    Present,
    Unexpected,
}

/// A decision permitted by exactly one truth-table row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryDecision {
    /// The physical mutation for the next checkpoint completed before the
    /// phase update was durable. Persist this phase, then evaluate again.
    AdvanceRecord {
        to: OperationPhase,
    },
    Rollback(RecoveryRollback),
    Forward(RecoveryForward),
    Conflict(RecoveryConflict),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryRollback {
    DeleteRecordOnly,
    RemoveOperationOwnedExclude,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryForward {
    MaterializeRepo,
    SaveManifest,
    VerifyPostconditionsAndComplete,
    StageStoreIntoBackup,
    RemoveManifest,
    ApplyKeepIgnorePolicy,
    DeleteBackupAndComplete,
    MoveRepoMaterialization,
    RemoveOldExclude,
    AttachOwnership,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryConflict {
    UnsupportedOperation,
    FactsDoNotMatchOperation,
    FactCollectionUnavailable,
    DirectionRequired,
    UnsupportedPhase,
    UnexpectedPhysicalState,
}

impl RecoveryConflict {
    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::UnsupportedOperation => "operation has no approved durable recovery table",
            Self::FactsDoNotMatchOperation => "recovery facts do not match the recorded operation",
            Self::FactCollectionUnavailable => {
                "operation has not yet persisted the observations required by its recovery table"
            }
            Self::DirectionRequired => "durable relink recovery requires an explicit direction",
            Self::UnsupportedPhase => "phase is not valid for this operation's recovery table",
            Self::UnexpectedPhysicalState => {
                "physical state differs from the current or exactly-next recovery checkpoint"
            }
        }
    }
}

/// Evaluates the approved recovery truth table for one record.
///
/// A physical state may match the recorded phase or exactly the following
/// phase.  The latter is the only allowed skipped-record state and is handled
/// by `AdvanceRecord`; accepting two or more skipped phases would silently
/// bypass a durable boundary.
pub(crate) fn decide(input: RecoveryPolicyInput) -> RecoveryDecision {
    match (input.operation, input.facts) {
        (OperationKind::Add, RecoveryFacts::Add(facts)) => decide_add(input.phase, facts),
        (OperationKind::Restore, RecoveryFacts::Restore(facts)) => {
            decide_restore(input.phase, facts)
        }
        (OperationKind::Move, RecoveryFacts::Move(facts)) => decide_move(input.phase, facts),
        (OperationKind::Relink, RecoveryFacts::DirectionalRelink(facts)) => {
            if input.direction.is_none() {
                RecoveryDecision::Conflict(RecoveryConflict::DirectionRequired)
            } else {
                decide_directional_relink(input.phase, facts)
            }
        }
        (
            OperationKind::Add
            | OperationKind::Restore
            | OperationKind::Move
            | OperationKind::Relink,
            _,
        ) => RecoveryDecision::Conflict(RecoveryConflict::FactsDoNotMatchOperation),
        (OperationKind::Sync | OperationKind::Repair, _) => {
            RecoveryDecision::Conflict(RecoveryConflict::UnsupportedOperation)
        }
    }
}

/// Produces the fail-closed decision used for records written before their
/// operation migration has supplied a fact collector.
///
/// The Phase 6 table is still evaluated with explicitly unexpected facts so
/// the production recovery gate exercises the same typed path as migrated
/// operations.  No recovery action is exposed until the operation can supply
/// identity- and ownership-verified observations.
pub(crate) fn decision_without_observations(
    operation: OperationKind,
    phase: OperationPhase,
    direction: Option<OperationDirection>,
) -> RecoveryDecision {
    let facts = match operation {
        OperationKind::Add => RecoveryFacts::Add(AddRecoveryFacts {
            repo: AddRepoFact::Unexpected,
            store: AddStoreFact::Unexpected,
            manifest: AddManifestFact::Unexpected,
            exclude: AddExcludeFact::Unexpected,
        }),
        OperationKind::Restore => RecoveryFacts::Restore(RestoreRecoveryFacts {
            repo: RestoreRepoFact::Unexpected,
            store: RestoreStoreFact::Unexpected,
            manifest: RestoreManifestFact::Unexpected,
            exclude: RestoreExcludeFact::Unexpected,
        }),
        OperationKind::Move => RecoveryFacts::Move(MoveRecoveryFacts {
            repo: MoveRepoFact::Unexpected,
            store: MoveStoreFact::Unexpected,
            manifest: MoveManifestFact::Unexpected,
            exclude: MoveExcludeFact::Unexpected,
        }),
        OperationKind::Relink => RecoveryFacts::DirectionalRelink(DirectionalRelinkRecoveryFacts {
            content: DirectionalRelinkContentFact::Unexpected,
            manifest: DirectionalRelinkManifestFact::Unexpected,
            exclude: DirectionalRelinkExcludeFact::Unexpected,
        }),
        // These operations intentionally have no durable operation table.
        OperationKind::Sync | OperationKind::Repair => RecoveryFacts::Add(AddRecoveryFacts {
            repo: AddRepoFact::Unexpected,
            store: AddStoreFact::Unexpected,
            manifest: AddManifestFact::Unexpected,
            exclude: AddExcludeFact::Unexpected,
        }),
    };

    match decide(RecoveryPolicyInput {
        operation,
        phase,
        direction,
        facts,
    }) {
        RecoveryDecision::Conflict(RecoveryConflict::UnsupportedOperation) => {
            RecoveryDecision::Conflict(RecoveryConflict::UnsupportedOperation)
        }
        RecoveryDecision::Conflict(_) => {
            RecoveryDecision::Conflict(RecoveryConflict::FactCollectionUnavailable)
        }
        // The all-unexpected facts above must never match a row. Preserve the
        // safe failure mode if a future table accidentally weakens that rule.
        _ => RecoveryDecision::Conflict(RecoveryConflict::UnexpectedPhysicalState),
    }
}

#[derive(Clone, Copy)]
struct RecoveryRow<F> {
    phase: OperationPhase,
    expected: F,
    action: RecoveryRowAction,
}

#[derive(Debug, Clone, Copy)]
enum RecoveryRowAction {
    Rollback(RecoveryRollback),
    Forward(RecoveryForward),
}

fn decide_table<F: Copy + PartialEq>(
    recorded_phase: OperationPhase,
    facts: F,
    rows: &[RecoveryRow<F>],
) -> RecoveryDecision {
    let Some(index) = rows.iter().position(|row| row.phase == recorded_phase) else {
        return RecoveryDecision::Conflict(RecoveryConflict::UnsupportedPhase);
    };

    let current = rows[index];
    if facts == current.expected {
        return match current.action {
            RecoveryRowAction::Rollback(action) => RecoveryDecision::Rollback(action),
            RecoveryRowAction::Forward(action) => RecoveryDecision::Forward(action),
        };
    }

    if let Some(next) = rows.get(index + 1) {
        if facts == next.expected {
            return RecoveryDecision::AdvanceRecord { to: next.phase };
        }
    }

    RecoveryDecision::Conflict(RecoveryConflict::UnexpectedPhysicalState)
}

fn decide_add(phase: OperationPhase, facts: AddRecoveryFacts) -> RecoveryDecision {
    decide_table(
        phase,
        facts,
        &[
            RecoveryRow {
                phase: OperationPhase::RecordCreated,
                expected: AddRecoveryFacts {
                    repo: AddRepoFact::Source,
                    store: AddStoreFact::Missing,
                    manifest: AddManifestFact::Absent,
                    exclude: AddExcludeFact::BeforeState,
                },
                action: RecoveryRowAction::Rollback(RecoveryRollback::DeleteRecordOnly),
            },
            RecoveryRow {
                phase: OperationPhase::ExcludeWritten,
                expected: AddRecoveryFacts {
                    repo: AddRepoFact::Source,
                    store: AddStoreFact::Missing,
                    manifest: AddManifestFact::Absent,
                    exclude: AddExcludeFact::Present,
                },
                action: RecoveryRowAction::Rollback(RecoveryRollback::RemoveOperationOwnedExclude),
            },
            RecoveryRow {
                phase: OperationPhase::StoreTransferred,
                expected: AddRecoveryFacts {
                    repo: AddRepoFact::Missing,
                    store: AddStoreFact::Source,
                    manifest: AddManifestFact::Absent,
                    exclude: AddExcludeFact::Present,
                },
                action: RecoveryRowAction::Forward(RecoveryForward::MaterializeRepo),
            },
            RecoveryRow {
                phase: OperationPhase::RepoMaterialized,
                expected: AddRecoveryFacts {
                    repo: AddRepoFact::Materialized,
                    store: AddStoreFact::Source,
                    manifest: AddManifestFact::Absent,
                    exclude: AddExcludeFact::Present,
                },
                action: RecoveryRowAction::Forward(RecoveryForward::SaveManifest),
            },
            RecoveryRow {
                phase: OperationPhase::ManifestSaved,
                expected: AddRecoveryFacts {
                    repo: AddRepoFact::Materialized,
                    store: AddStoreFact::Source,
                    manifest: AddManifestFact::Present,
                    exclude: AddExcludeFact::Present,
                },
                action: RecoveryRowAction::Forward(
                    RecoveryForward::VerifyPostconditionsAndComplete,
                ),
            },
        ],
    )
}

fn decide_restore(phase: OperationPhase, facts: RestoreRecoveryFacts) -> RecoveryDecision {
    decide_table(
        phase,
        facts,
        &[
            RecoveryRow {
                phase: OperationPhase::RecordCreated,
                expected: RestoreRecoveryFacts {
                    repo: RestoreRepoFact::OriginalMaterialization,
                    store: RestoreStoreFact::CanonicalPresent,
                    manifest: RestoreManifestFact::Present,
                    exclude: RestoreExcludeFact::BeforeState,
                },
                action: RecoveryRowAction::Rollback(RecoveryRollback::DeleteRecordOnly),
            },
            RecoveryRow {
                phase: OperationPhase::RepoRegularized,
                expected: RestoreRecoveryFacts {
                    repo: RestoreRepoFact::RegularMatchingCanonical,
                    store: RestoreStoreFact::CanonicalPresent,
                    manifest: RestoreManifestFact::Present,
                    exclude: RestoreExcludeFact::BeforeState,
                },
                action: RecoveryRowAction::Forward(RecoveryForward::StageStoreIntoBackup),
            },
            RecoveryRow {
                phase: OperationPhase::StoreStaged,
                expected: RestoreRecoveryFacts {
                    repo: RestoreRepoFact::RegularMatchingCanonical,
                    store: RestoreStoreFact::CanonicalMissingBackupPresent,
                    manifest: RestoreManifestFact::Present,
                    exclude: RestoreExcludeFact::BeforeState,
                },
                action: RecoveryRowAction::Forward(RecoveryForward::RemoveManifest),
            },
            RecoveryRow {
                phase: OperationPhase::ManifestRemoved,
                expected: RestoreRecoveryFacts {
                    repo: RestoreRepoFact::RegularMatchingCanonical,
                    store: RestoreStoreFact::CanonicalMissingBackupPresent,
                    manifest: RestoreManifestFact::Absent,
                    exclude: RestoreExcludeFact::BeforeState,
                },
                action: RecoveryRowAction::Forward(RecoveryForward::ApplyKeepIgnorePolicy),
            },
            RecoveryRow {
                phase: OperationPhase::ExcludeUpdated,
                expected: RestoreRecoveryFacts {
                    repo: RestoreRepoFact::RegularMatchingCanonical,
                    store: RestoreStoreFact::CanonicalMissingBackupPresent,
                    manifest: RestoreManifestFact::Absent,
                    exclude: RestoreExcludeFact::FinalState,
                },
                action: RecoveryRowAction::Forward(RecoveryForward::DeleteBackupAndComplete),
            },
        ],
    )
}

fn decide_move(phase: OperationPhase, facts: MoveRecoveryFacts) -> RecoveryDecision {
    decide_table(
        phase,
        facts,
        &[
            RecoveryRow {
                phase: OperationPhase::RecordCreated,
                expected: MoveRecoveryFacts {
                    repo: MoveRepoFact::OldMaterializedNewMissing,
                    store: MoveStoreFact::OldPresentNewMissing,
                    manifest: MoveManifestFact::OldPath,
                    exclude: MoveExcludeFact::BeforeState,
                },
                action: RecoveryRowAction::Rollback(RecoveryRollback::DeleteRecordOnly),
            },
            RecoveryRow {
                phase: OperationPhase::DestinationExcluded,
                expected: MoveRecoveryFacts {
                    repo: MoveRepoFact::OldMaterializedNewMissing,
                    store: MoveStoreFact::OldPresentNewMissing,
                    manifest: MoveManifestFact::OldPath,
                    exclude: MoveExcludeFact::OldAndNew,
                },
                action: RecoveryRowAction::Rollback(RecoveryRollback::RemoveOperationOwnedExclude),
            },
            RecoveryRow {
                phase: OperationPhase::StoreTransferred,
                expected: MoveRecoveryFacts {
                    repo: MoveRepoFact::OldMaterializedNewMissing,
                    store: MoveStoreFact::OldMissingNewPresent,
                    manifest: MoveManifestFact::OldPath,
                    exclude: MoveExcludeFact::OldAndNew,
                },
                action: RecoveryRowAction::Forward(RecoveryForward::MoveRepoMaterialization),
            },
            RecoveryRow {
                phase: OperationPhase::RepoMoved,
                expected: MoveRecoveryFacts {
                    repo: MoveRepoFact::OldMissingNewMaterialized,
                    store: MoveStoreFact::OldMissingNewPresent,
                    manifest: MoveManifestFact::OldPath,
                    exclude: MoveExcludeFact::OldAndNew,
                },
                action: RecoveryRowAction::Forward(RecoveryForward::SaveManifest),
            },
            RecoveryRow {
                phase: OperationPhase::ManifestSaved,
                expected: MoveRecoveryFacts {
                    repo: MoveRepoFact::OldMissingNewMaterialized,
                    store: MoveStoreFact::OldMissingNewPresent,
                    manifest: MoveManifestFact::NewPath,
                    exclude: MoveExcludeFact::OldAndNew,
                },
                action: RecoveryRowAction::Forward(RecoveryForward::RemoveOldExclude),
            },
            RecoveryRow {
                phase: OperationPhase::ExcludeFinalized,
                expected: MoveRecoveryFacts {
                    repo: MoveRepoFact::OldMissingNewMaterialized,
                    store: MoveStoreFact::OldMissingNewPresent,
                    manifest: MoveManifestFact::NewPath,
                    exclude: MoveExcludeFact::NewOnly,
                },
                action: RecoveryRowAction::Forward(
                    RecoveryForward::VerifyPostconditionsAndComplete,
                ),
            },
        ],
    )
}

fn decide_directional_relink(
    phase: OperationPhase,
    facts: DirectionalRelinkRecoveryFacts,
) -> RecoveryDecision {
    decide_table(
        phase,
        facts,
        &[
            RecoveryRow {
                phase: OperationPhase::RecordCreated,
                expected: DirectionalRelinkRecoveryFacts {
                    content: DirectionalRelinkContentFact::BeforeDirectionBackupAbsent,
                    manifest: DirectionalRelinkManifestFact::Detached,
                    exclude: DirectionalRelinkExcludeFact::BeforeState,
                },
                action: RecoveryRowAction::Rollback(RecoveryRollback::DeleteRecordOnly),
            },
            RecoveryRow {
                phase: OperationPhase::ExcludeWritten,
                expected: DirectionalRelinkRecoveryFacts {
                    content: DirectionalRelinkContentFact::BeforeDirectionBackupAbsent,
                    manifest: DirectionalRelinkManifestFact::Detached,
                    exclude: DirectionalRelinkExcludeFact::Present,
                },
                action: RecoveryRowAction::Rollback(RecoveryRollback::RemoveOperationOwnedExclude),
            },
            RecoveryRow {
                phase: OperationPhase::ContentSynchronized,
                expected: DirectionalRelinkRecoveryFacts {
                    content: DirectionalRelinkContentFact::SynchronizedBackupPresent,
                    manifest: DirectionalRelinkManifestFact::Detached,
                    exclude: DirectionalRelinkExcludeFact::Present,
                },
                action: RecoveryRowAction::Forward(RecoveryForward::AttachOwnership),
            },
            RecoveryRow {
                phase: OperationPhase::OwnershipAttached,
                expected: DirectionalRelinkRecoveryFacts {
                    content: DirectionalRelinkContentFact::SynchronizedBackupPresent,
                    manifest: DirectionalRelinkManifestFact::Attached,
                    exclude: DirectionalRelinkExcludeFact::Present,
                },
                action: RecoveryRowAction::Forward(RecoveryForward::DeleteBackupAndComplete),
            },
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADD_ROWS: &[(OperationPhase, AddRecoveryFacts, RecoveryDecision)] = &[
        (
            OperationPhase::RecordCreated,
            AddRecoveryFacts {
                repo: AddRepoFact::Source,
                store: AddStoreFact::Missing,
                manifest: AddManifestFact::Absent,
                exclude: AddExcludeFact::BeforeState,
            },
            RecoveryDecision::Rollback(RecoveryRollback::DeleteRecordOnly),
        ),
        (
            OperationPhase::ExcludeWritten,
            AddRecoveryFacts {
                repo: AddRepoFact::Source,
                store: AddStoreFact::Missing,
                manifest: AddManifestFact::Absent,
                exclude: AddExcludeFact::Present,
            },
            RecoveryDecision::Rollback(RecoveryRollback::RemoveOperationOwnedExclude),
        ),
        (
            OperationPhase::StoreTransferred,
            AddRecoveryFacts {
                repo: AddRepoFact::Missing,
                store: AddStoreFact::Source,
                manifest: AddManifestFact::Absent,
                exclude: AddExcludeFact::Present,
            },
            RecoveryDecision::Forward(RecoveryForward::MaterializeRepo),
        ),
        (
            OperationPhase::RepoMaterialized,
            AddRecoveryFacts {
                repo: AddRepoFact::Materialized,
                store: AddStoreFact::Source,
                manifest: AddManifestFact::Absent,
                exclude: AddExcludeFact::Present,
            },
            RecoveryDecision::Forward(RecoveryForward::SaveManifest),
        ),
        (
            OperationPhase::ManifestSaved,
            AddRecoveryFacts {
                repo: AddRepoFact::Materialized,
                store: AddStoreFact::Source,
                manifest: AddManifestFact::Present,
                exclude: AddExcludeFact::Present,
            },
            RecoveryDecision::Forward(RecoveryForward::VerifyPostconditionsAndComplete),
        ),
    ];

    const RESTORE_ROWS: &[(OperationPhase, RestoreRecoveryFacts, RecoveryDecision)] = &[
        (
            OperationPhase::RecordCreated,
            RestoreRecoveryFacts {
                repo: RestoreRepoFact::OriginalMaterialization,
                store: RestoreStoreFact::CanonicalPresent,
                manifest: RestoreManifestFact::Present,
                exclude: RestoreExcludeFact::BeforeState,
            },
            RecoveryDecision::Rollback(RecoveryRollback::DeleteRecordOnly),
        ),
        (
            OperationPhase::RepoRegularized,
            RestoreRecoveryFacts {
                repo: RestoreRepoFact::RegularMatchingCanonical,
                store: RestoreStoreFact::CanonicalPresent,
                manifest: RestoreManifestFact::Present,
                exclude: RestoreExcludeFact::BeforeState,
            },
            RecoveryDecision::Forward(RecoveryForward::StageStoreIntoBackup),
        ),
        (
            OperationPhase::StoreStaged,
            RestoreRecoveryFacts {
                repo: RestoreRepoFact::RegularMatchingCanonical,
                store: RestoreStoreFact::CanonicalMissingBackupPresent,
                manifest: RestoreManifestFact::Present,
                exclude: RestoreExcludeFact::BeforeState,
            },
            RecoveryDecision::Forward(RecoveryForward::RemoveManifest),
        ),
        (
            OperationPhase::ManifestRemoved,
            RestoreRecoveryFacts {
                repo: RestoreRepoFact::RegularMatchingCanonical,
                store: RestoreStoreFact::CanonicalMissingBackupPresent,
                manifest: RestoreManifestFact::Absent,
                exclude: RestoreExcludeFact::BeforeState,
            },
            RecoveryDecision::Forward(RecoveryForward::ApplyKeepIgnorePolicy),
        ),
        (
            OperationPhase::ExcludeUpdated,
            RestoreRecoveryFacts {
                repo: RestoreRepoFact::RegularMatchingCanonical,
                store: RestoreStoreFact::CanonicalMissingBackupPresent,
                manifest: RestoreManifestFact::Absent,
                exclude: RestoreExcludeFact::FinalState,
            },
            RecoveryDecision::Forward(RecoveryForward::DeleteBackupAndComplete),
        ),
    ];

    const MOVE_ROWS: &[(OperationPhase, MoveRecoveryFacts, RecoveryDecision)] = &[
        (
            OperationPhase::RecordCreated,
            MoveRecoveryFacts {
                repo: MoveRepoFact::OldMaterializedNewMissing,
                store: MoveStoreFact::OldPresentNewMissing,
                manifest: MoveManifestFact::OldPath,
                exclude: MoveExcludeFact::BeforeState,
            },
            RecoveryDecision::Rollback(RecoveryRollback::DeleteRecordOnly),
        ),
        (
            OperationPhase::DestinationExcluded,
            MoveRecoveryFacts {
                repo: MoveRepoFact::OldMaterializedNewMissing,
                store: MoveStoreFact::OldPresentNewMissing,
                manifest: MoveManifestFact::OldPath,
                exclude: MoveExcludeFact::OldAndNew,
            },
            RecoveryDecision::Rollback(RecoveryRollback::RemoveOperationOwnedExclude),
        ),
        (
            OperationPhase::StoreTransferred,
            MoveRecoveryFacts {
                repo: MoveRepoFact::OldMaterializedNewMissing,
                store: MoveStoreFact::OldMissingNewPresent,
                manifest: MoveManifestFact::OldPath,
                exclude: MoveExcludeFact::OldAndNew,
            },
            RecoveryDecision::Forward(RecoveryForward::MoveRepoMaterialization),
        ),
        (
            OperationPhase::RepoMoved,
            MoveRecoveryFacts {
                repo: MoveRepoFact::OldMissingNewMaterialized,
                store: MoveStoreFact::OldMissingNewPresent,
                manifest: MoveManifestFact::OldPath,
                exclude: MoveExcludeFact::OldAndNew,
            },
            RecoveryDecision::Forward(RecoveryForward::SaveManifest),
        ),
        (
            OperationPhase::ManifestSaved,
            MoveRecoveryFacts {
                repo: MoveRepoFact::OldMissingNewMaterialized,
                store: MoveStoreFact::OldMissingNewPresent,
                manifest: MoveManifestFact::NewPath,
                exclude: MoveExcludeFact::OldAndNew,
            },
            RecoveryDecision::Forward(RecoveryForward::RemoveOldExclude),
        ),
        (
            OperationPhase::ExcludeFinalized,
            MoveRecoveryFacts {
                repo: MoveRepoFact::OldMissingNewMaterialized,
                store: MoveStoreFact::OldMissingNewPresent,
                manifest: MoveManifestFact::NewPath,
                exclude: MoveExcludeFact::NewOnly,
            },
            RecoveryDecision::Forward(RecoveryForward::VerifyPostconditionsAndComplete),
        ),
    ];

    const RELINK_ROWS: &[(
        OperationPhase,
        DirectionalRelinkRecoveryFacts,
        RecoveryDecision,
    )] = &[
        (
            OperationPhase::RecordCreated,
            DirectionalRelinkRecoveryFacts {
                content: DirectionalRelinkContentFact::BeforeDirectionBackupAbsent,
                manifest: DirectionalRelinkManifestFact::Detached,
                exclude: DirectionalRelinkExcludeFact::BeforeState,
            },
            RecoveryDecision::Rollback(RecoveryRollback::DeleteRecordOnly),
        ),
        (
            OperationPhase::ExcludeWritten,
            DirectionalRelinkRecoveryFacts {
                content: DirectionalRelinkContentFact::BeforeDirectionBackupAbsent,
                manifest: DirectionalRelinkManifestFact::Detached,
                exclude: DirectionalRelinkExcludeFact::Present,
            },
            RecoveryDecision::Rollback(RecoveryRollback::RemoveOperationOwnedExclude),
        ),
        (
            OperationPhase::ContentSynchronized,
            DirectionalRelinkRecoveryFacts {
                content: DirectionalRelinkContentFact::SynchronizedBackupPresent,
                manifest: DirectionalRelinkManifestFact::Detached,
                exclude: DirectionalRelinkExcludeFact::Present,
            },
            RecoveryDecision::Forward(RecoveryForward::AttachOwnership),
        ),
        (
            OperationPhase::OwnershipAttached,
            DirectionalRelinkRecoveryFacts {
                content: DirectionalRelinkContentFact::SynchronizedBackupPresent,
                manifest: DirectionalRelinkManifestFact::Attached,
                exclude: DirectionalRelinkExcludeFact::Present,
            },
            RecoveryDecision::Forward(RecoveryForward::DeleteBackupAndComplete),
        ),
    ];

    #[test]
    fn every_add_row_has_its_approved_decision_and_exactly_next_state_advances() {
        for (index, (phase, facts, expected)) in ADD_ROWS.iter().enumerate() {
            assert_eq!(
                decide(RecoveryPolicyInput {
                    operation: OperationKind::Add,
                    phase: *phase,
                    direction: None,
                    facts: RecoveryFacts::Add(*facts),
                }),
                *expected
            );
            if let Some((next_phase, next_facts, _)) = ADD_ROWS.get(index + 1) {
                assert_eq!(
                    decide(RecoveryPolicyInput {
                        operation: OperationKind::Add,
                        phase: *phase,
                        direction: None,
                        facts: RecoveryFacts::Add(*next_facts),
                    }),
                    RecoveryDecision::AdvanceRecord { to: *next_phase }
                );
            }
        }
    }

    #[test]
    fn every_restore_row_has_its_approved_decision_and_exactly_next_state_advances() {
        for (index, (phase, facts, expected)) in RESTORE_ROWS.iter().enumerate() {
            assert_eq!(
                decide(RecoveryPolicyInput {
                    operation: OperationKind::Restore,
                    phase: *phase,
                    direction: None,
                    facts: RecoveryFacts::Restore(*facts),
                }),
                *expected
            );
            if let Some((next_phase, next_facts, _)) = RESTORE_ROWS.get(index + 1) {
                assert_eq!(
                    decide(RecoveryPolicyInput {
                        operation: OperationKind::Restore,
                        phase: *phase,
                        direction: None,
                        facts: RecoveryFacts::Restore(*next_facts),
                    }),
                    RecoveryDecision::AdvanceRecord { to: *next_phase }
                );
            }
        }
    }

    #[test]
    fn every_move_row_has_its_approved_decision_and_exactly_next_state_advances() {
        for (index, (phase, facts, expected)) in MOVE_ROWS.iter().enumerate() {
            assert_eq!(
                decide(RecoveryPolicyInput {
                    operation: OperationKind::Move,
                    phase: *phase,
                    direction: None,
                    facts: RecoveryFacts::Move(*facts),
                }),
                *expected
            );
            if let Some((next_phase, next_facts, _)) = MOVE_ROWS.get(index + 1) {
                assert_eq!(
                    decide(RecoveryPolicyInput {
                        operation: OperationKind::Move,
                        phase: *phase,
                        direction: None,
                        facts: RecoveryFacts::Move(*next_facts),
                    }),
                    RecoveryDecision::AdvanceRecord { to: *next_phase }
                );
            }
        }
    }

    #[test]
    fn every_directional_relink_row_has_its_approved_decision_and_exactly_next_state_advances() {
        for (index, (phase, facts, expected)) in RELINK_ROWS.iter().enumerate() {
            assert_eq!(
                decide(RecoveryPolicyInput {
                    operation: OperationKind::Relink,
                    phase: *phase,
                    direction: Some(OperationDirection::FromStore),
                    facts: RecoveryFacts::DirectionalRelink(*facts),
                }),
                *expected
            );
            if let Some((next_phase, next_facts, _)) = RELINK_ROWS.get(index + 1) {
                assert_eq!(
                    decide(RecoveryPolicyInput {
                        operation: OperationKind::Relink,
                        phase: *phase,
                        direction: Some(OperationDirection::FromRepo),
                        facts: RecoveryFacts::DirectionalRelink(*next_facts),
                    }),
                    RecoveryDecision::AdvanceRecord { to: *next_phase }
                );
            }
        }
    }

    #[test]
    fn changed_identity_content_membership_or_exclude_is_a_conflict() {
        let add = ADD_ROWS[2].1;
        for facts in [
            AddRecoveryFacts {
                repo: AddRepoFact::Unexpected,
                ..add
            },
            AddRecoveryFacts {
                store: AddStoreFact::Unexpected,
                ..add
            },
            AddRecoveryFacts {
                manifest: AddManifestFact::Unexpected,
                ..add
            },
            AddRecoveryFacts {
                exclude: AddExcludeFact::Unexpected,
                ..add
            },
        ] {
            assert_eq!(
                decide(RecoveryPolicyInput {
                    operation: OperationKind::Add,
                    phase: OperationPhase::StoreTransferred,
                    direction: None,
                    facts: RecoveryFacts::Add(facts),
                }),
                RecoveryDecision::Conflict(RecoveryConflict::UnexpectedPhysicalState)
            );
        }

        let restore = RESTORE_ROWS[2].1;
        for facts in [
            RestoreRecoveryFacts {
                repo: RestoreRepoFact::Unexpected,
                ..restore
            },
            RestoreRecoveryFacts {
                store: RestoreStoreFact::Unexpected,
                ..restore
            },
            RestoreRecoveryFacts {
                manifest: RestoreManifestFact::Unexpected,
                ..restore
            },
            RestoreRecoveryFacts {
                exclude: RestoreExcludeFact::Unexpected,
                ..restore
            },
        ] {
            assert_eq!(
                decide(RecoveryPolicyInput {
                    operation: OperationKind::Restore,
                    phase: OperationPhase::StoreStaged,
                    direction: None,
                    facts: RecoveryFacts::Restore(facts),
                }),
                RecoveryDecision::Conflict(RecoveryConflict::UnexpectedPhysicalState)
            );
        }

        let move_facts = MOVE_ROWS[4].1;
        for facts in [
            MoveRecoveryFacts {
                repo: MoveRepoFact::Unexpected,
                ..move_facts
            },
            MoveRecoveryFacts {
                store: MoveStoreFact::Unexpected,
                ..move_facts
            },
            MoveRecoveryFacts {
                manifest: MoveManifestFact::Unexpected,
                ..move_facts
            },
            MoveRecoveryFacts {
                exclude: MoveExcludeFact::Unexpected,
                ..move_facts
            },
        ] {
            assert_eq!(
                decide(RecoveryPolicyInput {
                    operation: OperationKind::Move,
                    phase: OperationPhase::ManifestSaved,
                    direction: None,
                    facts: RecoveryFacts::Move(facts),
                }),
                RecoveryDecision::Conflict(RecoveryConflict::UnexpectedPhysicalState)
            );
        }

        let relink = RELINK_ROWS[3].1;
        for facts in [
            DirectionalRelinkRecoveryFacts {
                content: DirectionalRelinkContentFact::Unexpected,
                ..relink
            },
            DirectionalRelinkRecoveryFacts {
                manifest: DirectionalRelinkManifestFact::Unexpected,
                ..relink
            },
            DirectionalRelinkRecoveryFacts {
                exclude: DirectionalRelinkExcludeFact::Unexpected,
                ..relink
            },
        ] {
            assert_eq!(
                decide(RecoveryPolicyInput {
                    operation: OperationKind::Relink,
                    phase: OperationPhase::OwnershipAttached,
                    direction: Some(OperationDirection::FromRepo),
                    facts: RecoveryFacts::DirectionalRelink(facts),
                }),
                RecoveryDecision::Conflict(RecoveryConflict::UnexpectedPhysicalState)
            );
        }

        assert_eq!(
            RecoveryConflict::UnexpectedPhysicalState.reason(),
            "physical state differs from the current or exactly-next recovery checkpoint"
        );
    }

    #[test]
    fn unlisted_or_ambiguous_state_never_selects_a_recovery_direction() {
        assert_eq!(
            decide(RecoveryPolicyInput {
                operation: OperationKind::Relink,
                phase: OperationPhase::RecordCreated,
                direction: None,
                facts: RecoveryFacts::DirectionalRelink(RELINK_ROWS[0].1),
            }),
            RecoveryDecision::Conflict(RecoveryConflict::DirectionRequired)
        );
        assert_eq!(
            decide(RecoveryPolicyInput {
                operation: OperationKind::Add,
                phase: OperationPhase::RepoRegularized,
                direction: None,
                facts: RecoveryFacts::Add(ADD_ROWS[0].1),
            }),
            RecoveryDecision::Conflict(RecoveryConflict::UnsupportedPhase)
        );
        assert_eq!(
            decide(RecoveryPolicyInput {
                operation: OperationKind::Sync,
                phase: OperationPhase::RecordCreated,
                direction: Some(OperationDirection::FromStore),
                facts: RecoveryFacts::Add(ADD_ROWS[0].1),
            }),
            RecoveryDecision::Conflict(RecoveryConflict::UnsupportedOperation)
        );
    }
}
