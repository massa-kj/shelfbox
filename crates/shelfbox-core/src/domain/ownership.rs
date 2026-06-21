use serde::{Deserialize, Serialize};

/// Ownership state of a shelved item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipState {
    /// Active owner; symlink valid or repairable.
    Attached,
    /// Intentionally unlinked via `restore --keep-store`; store item retained.
    Detached,
    /// Manifest exists, but no current Git clone is associated with the RepoId.
    Unreachable,
    /// No deterministic claimant; eligible for confirmed conservative GC.
    Orphaned,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_state_json_shape_is_snake_case() {
        assert_eq!(
            serde_json::to_string(&OwnershipState::Attached).unwrap(),
            "\"attached\""
        );
        assert_eq!(
            serde_json::to_string(&OwnershipState::Detached).unwrap(),
            "\"detached\""
        );
        assert_eq!(
            serde_json::to_string(&OwnershipState::Unreachable).unwrap(),
            "\"unreachable\""
        );
        assert_eq!(
            serde_json::to_string(&OwnershipState::Orphaned).unwrap(),
            "\"orphaned\""
        );
    }

    #[test]
    fn ownership_state_loads_legacy_manifest_values() {
        assert_eq!(
            serde_json::from_str::<OwnershipState>("\"attached\"").unwrap(),
            OwnershipState::Attached
        );
        assert_eq!(
            serde_json::from_str::<OwnershipState>("\"detached\"").unwrap(),
            OwnershipState::Detached
        );
        assert_eq!(
            serde_json::from_str::<OwnershipState>("\"unreachable\"").unwrap(),
            OwnershipState::Unreachable
        );
        assert_eq!(
            serde_json::from_str::<OwnershipState>("\"orphaned\"").unwrap(),
            OwnershipState::Orphaned
        );
    }
}
