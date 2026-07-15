//! User-selected durability contract for shelf mutations.
//!
//! This is intentionally independent from materialization.  It is persisted
//! in every recovery record so that recovery can retain the contract under
//! which the interrupted operation started.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The parent-directory durability contract for shelf mutations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MutationDurability {
    /// Require directory durability at every D5 protocol point.
    #[default]
    Require,
    /// Continue only when directory durability is the known unavailable
    /// platform capability. Other filesystem failures still propagate.
    BestEffort,
}

impl fmt::Display for MutationDurability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Require => f.write_str("require"),
            Self::BestEffort => f.write_str("best-effort"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_require_and_wire_values_are_stable() {
        assert_eq!(MutationDurability::default(), MutationDurability::Require);
        assert_eq!(
            serde_json::to_string(&MutationDurability::BestEffort).unwrap(),
            "\"best-effort\""
        );
        assert!(serde_json::from_str::<MutationDurability>("\"fallback\"").is_err());
    }
}
