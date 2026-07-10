//! Strategy-neutral materialization facts and presentation vocabulary.
//!
//! Filesystem adapters collect the facts in this module without deciding
//! whether an operation is allowed. Policy turns those facts into actions and
//! status. Keeping the vocabulary here prevents individual operations from
//! growing their own, subtly different symlink/copy state machines.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The strategy selected when a new repository materialization must be made.
///
/// This is a default, not persistent item state: existing materializations
/// are classified from observed facts instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MaterializationStrategy {
    #[default]
    Symlink,
    Copy,
}

impl MaterializationStrategy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Symlink => "symlink",
            Self::Copy => "copy",
        }
    }
}

impl fmt::Display for MaterializationStrategy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The non-following filesystem classification of the repository path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoEntryKind {
    Missing,
    RegularFile,
    Symlink,
    Unsupported,
    Unreadable,
}

/// The safety relationship between a repository entry and its canonical store
/// entry. This deliberately does not include Git or exclude facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterializationRelation {
    ManagedSymlink,
    IsolatedRegularCopy,
    UnsafeHardlink,
    UnexpectedSymlink,
    NotApplicable,
    InspectionFailed,
}

/// Result of a bounded-memory, byte-for-byte comparison for a regular copy.
///
/// `NotCompared` is distinct from every failed comparison and from a missing
/// store. The latter is represented by [`StoreState::Missing`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyContentState {
    NotCompared,
    Equal,
    Diverged,
    Unreadable,
    ComparisonFailed,
}

/// Availability and safe inspectability of the canonical store entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreState {
    Present,
    Missing,
    Unsupported,
    Unreadable,
}

/// Result of querying Git tracking state for a materialized path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitState {
    Untracked,
    Tracked,
    QueryFailed,
}

/// Result of querying the exact Git exclude entry for a materialized path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExcludeState {
    Present,
    Missing,
    QueryFailed,
}

/// Independently collected inputs to materialization policy.
///
/// No field is inferred from another field. For example, a missing store is
/// represented by [`store_state`](Self::store_state), rather than by a content
/// comparison failure. This shape is intentionally suitable for read-only
/// inspection and must not carry mutation handles or filesystem paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MaterializationFacts {
    pub repo_entry: RepoEntryKind,
    pub relation: MaterializationRelation,
    pub copy_content: CopyContentState,
    pub store_state: StoreState,
    pub git_state: GitState,
    pub exclude_state: ExcludeState,
}

impl MaterializationFacts {
    /// Projects independent facts into a stable display/API classification.
    ///
    /// Policy still evaluates content, Git, and exclude facts separately. A
    /// `RegularCopy`, for example, can be healthy or diverged.
    pub const fn observed_materialization(self) -> ObservedMaterialization {
        match self.repo_entry {
            RepoEntryKind::Missing => ObservedMaterialization::Missing,
            RepoEntryKind::Unreadable => ObservedMaterialization::Unreadable,
            RepoEntryKind::Unsupported => ObservedMaterialization::UnsupportedEntry,
            RepoEntryKind::Symlink => match self.relation {
                MaterializationRelation::ManagedSymlink => ObservedMaterialization::ManagedSymlink,
                MaterializationRelation::InspectionFailed => ObservedMaterialization::Unreadable,
                MaterializationRelation::UnexpectedSymlink
                | MaterializationRelation::UnsafeHardlink
                | MaterializationRelation::IsolatedRegularCopy
                | MaterializationRelation::NotApplicable => {
                    ObservedMaterialization::UnexpectedSymlink
                }
            },
            RepoEntryKind::RegularFile => match self.relation {
                MaterializationRelation::IsolatedRegularCopy => {
                    ObservedMaterialization::RegularCopy
                }
                MaterializationRelation::UnsafeHardlink => ObservedMaterialization::UnsafeHardlink,
                MaterializationRelation::InspectionFailed => ObservedMaterialization::Unreadable,
                MaterializationRelation::ManagedSymlink
                | MaterializationRelation::UnexpectedSymlink
                | MaterializationRelation::NotApplicable => {
                    ObservedMaterialization::UnsupportedEntry
                }
            },
        }
    }
}

/// Presentation classification derived from [`MaterializationFacts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedMaterialization {
    Missing,
    ManagedSymlink,
    RegularCopy,
    UnsafeHardlink,
    UnexpectedSymlink,
    UnsupportedEntry,
    Unreadable,
}

/// Overall integrity severity after materialization policy evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusSeverity {
    Healthy,
    Warning,
    Error,
}

/// Stable machine-readable integrity issue codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusIssueCode {
    MaterializationMissing,
    MaterializationInvalid,
    StoreMissing,
    MissingExclude,
    TrackedByGit,
    ContentDiverged,
    ContentUnreadable,
    HardlinkUnsafe,
    PathEscape,
    UnfinishedOperationConflict,
}

/// Stable machine-readable informational status note codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusNoteCode {
    StrategyMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusIssue {
    pub code: StatusIssueCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusNote {
    pub code: StatusNoteCode,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn strategy_uses_stable_config_values_and_defaults_to_symlink() {
        assert_eq!(
            MaterializationStrategy::default(),
            MaterializationStrategy::Symlink
        );
        assert_eq!(MaterializationStrategy::Copy.to_string(), "copy");
        assert_eq!(
            serde_json::to_value(MaterializationStrategy::Symlink).unwrap(),
            json!("symlink")
        );
    }

    #[test]
    fn facts_project_each_safe_observed_materialization() {
        let base = MaterializationFacts {
            repo_entry: RepoEntryKind::RegularFile,
            relation: MaterializationRelation::IsolatedRegularCopy,
            copy_content: CopyContentState::Equal,
            store_state: StoreState::Present,
            git_state: GitState::Untracked,
            exclude_state: ExcludeState::Present,
        };

        assert_eq!(
            base.observed_materialization(),
            ObservedMaterialization::RegularCopy
        );
        assert_eq!(
            MaterializationFacts {
                repo_entry: RepoEntryKind::Symlink,
                relation: MaterializationRelation::ManagedSymlink,
                ..base
            }
            .observed_materialization(),
            ObservedMaterialization::ManagedSymlink
        );
        assert_eq!(
            MaterializationFacts {
                relation: MaterializationRelation::UnsafeHardlink,
                ..base
            }
            .observed_materialization(),
            ObservedMaterialization::UnsafeHardlink
        );
        assert_eq!(
            MaterializationFacts {
                repo_entry: RepoEntryKind::Unreadable,
                ..base
            }
            .observed_materialization(),
            ObservedMaterialization::Unreadable
        );
    }

    #[test]
    fn missing_store_is_not_a_content_state() {
        let facts = MaterializationFacts {
            repo_entry: RepoEntryKind::RegularFile,
            relation: MaterializationRelation::IsolatedRegularCopy,
            copy_content: CopyContentState::NotCompared,
            store_state: StoreState::Missing,
            git_state: GitState::Untracked,
            exclude_state: ExcludeState::Present,
        };

        assert_eq!(facts.store_state, StoreState::Missing);
        assert_eq!(facts.copy_content, CopyContentState::NotCompared);
    }
}
