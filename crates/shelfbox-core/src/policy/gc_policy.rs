use crate::domain::ownership::OwnershipState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GcProtection {
    Collectible,
    Attached,
    Detached,
    Unreachable,
}

pub(crate) fn classify_ownership(state: OwnershipState) -> GcProtection {
    match state {
        OwnershipState::Orphaned => GcProtection::Collectible,
        OwnershipState::Attached => GcProtection::Attached,
        OwnershipState::Detached => GcProtection::Detached,
        OwnershipState::Unreachable => GcProtection::Unreachable,
    }
}

pub(crate) fn is_collectible(state: OwnershipState) -> bool {
    classify_ownership(state) == GcProtection::Collectible
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_orphaned_items_are_collectible() {
        assert!(is_collectible(OwnershipState::Orphaned));
        assert!(!is_collectible(OwnershipState::Attached));
        assert!(!is_collectible(OwnershipState::Detached));
        assert!(!is_collectible(OwnershipState::Unreachable));
    }

    #[test]
    fn protected_states_keep_their_reasons() {
        assert_eq!(
            classify_ownership(OwnershipState::Attached),
            GcProtection::Attached
        );
        assert_eq!(
            classify_ownership(OwnershipState::Detached),
            GcProtection::Detached
        );
        assert_eq!(
            classify_ownership(OwnershipState::Unreachable),
            GcProtection::Unreachable
        );
    }
}
