//! Pure eligibility and action policy for explicit item synchronization.

use crate::{
    domain::materialization::CopyContentState,
    plan::item_sync::{ItemSyncAction, SyncDirection},
};

/// The operation-relevant materialization state after all unsafe, Git, and
/// ownership states have been rejected by the operation layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncMaterializationState {
    ManagedSymlink,
    RegularCopy(CopyContentState),
    Missing,
    Unsafe,
}

/// The policy result before the operation maps invalid states to a typed,
/// actionable error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncDecision {
    Action(ItemSyncAction),
    MissingNeedsRepair,
    RequiresAttachedRegularCopy,
    InspectionFailed,
}

pub(crate) fn decide_sync(
    direction: SyncDirection,
    state: SyncMaterializationState,
) -> SyncDecision {
    match (direction, state) {
        (SyncDirection::FromStore, SyncMaterializationState::ManagedSymlink) => {
            SyncDecision::Action(ItemSyncAction::ManagedSymlinkNoOp)
        }
        (
            SyncDirection::FromStore,
            SyncMaterializationState::RegularCopy(CopyContentState::Equal),
        ) => SyncDecision::Action(ItemSyncAction::AlreadySynchronized),
        (
            SyncDirection::FromStore,
            SyncMaterializationState::RegularCopy(CopyContentState::Diverged),
        ) => SyncDecision::Action(ItemSyncAction::ReplaceRepoFromStore),
        (SyncDirection::FromStore, SyncMaterializationState::Missing) => {
            SyncDecision::MissingNeedsRepair
        }
        (
            SyncDirection::FromRepo,
            SyncMaterializationState::RegularCopy(CopyContentState::Equal),
        ) => SyncDecision::Action(ItemSyncAction::AlreadySynchronized),
        (
            SyncDirection::FromRepo,
            SyncMaterializationState::RegularCopy(CopyContentState::Diverged),
        ) => SyncDecision::Action(ItemSyncAction::ReplaceStoreFromRepo),
        (SyncDirection::FromRepo, SyncMaterializationState::Missing) => {
            SyncDecision::MissingNeedsRepair
        }
        (SyncDirection::FromRepo, SyncMaterializationState::ManagedSymlink) => {
            SyncDecision::RequiresAttachedRegularCopy
        }
        (_, SyncMaterializationState::Unsafe) => SyncDecision::InspectionFailed,
        (_, SyncMaterializationState::RegularCopy(_)) => SyncDecision::InspectionFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_directions_select_their_respective_overwrite() {
        assert_eq!(
            decide_sync(
                SyncDirection::FromStore,
                SyncMaterializationState::RegularCopy(CopyContentState::Diverged),
            ),
            SyncDecision::Action(ItemSyncAction::ReplaceRepoFromStore)
        );
        assert_eq!(
            decide_sync(
                SyncDirection::FromRepo,
                SyncMaterializationState::RegularCopy(CopyContentState::Diverged),
            ),
            SyncDecision::Action(ItemSyncAction::ReplaceStoreFromRepo)
        );
    }

    #[test]
    fn missing_never_selects_a_strategy_or_overwrite() {
        for direction in [SyncDirection::FromStore, SyncDirection::FromRepo] {
            assert_eq!(
                decide_sync(direction, SyncMaterializationState::Missing),
                SyncDecision::MissingNeedsRepair
            );
        }
    }
}
