//! Shared, side-effect-free materialization status policy.
//!
//! Filesystem, Git, and exclude adapters collect independent facts; this
//! module is the sole owner of their health classification. In particular, a
//! configured strategy is a default for future writes, not a requirement that
//! existing healthy items be converted.

use crate::domain::materialization::{
    CopyContentState, ExcludeState, GitState, MaterializationFacts, MaterializationStrategy,
    ObservedMaterialization, StatusIssue, StatusIssueCode, StatusNote, StatusNoteCode,
    StatusSeverity, StoreState,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MaterializationStatus {
    pub observed: ObservedMaterialization,
    pub materialization_exists: bool,
    pub materialization_valid: bool,
    pub severity: StatusSeverity,
    pub issues: Vec<StatusIssue>,
    pub notes: Vec<StatusNote>,
}

pub(crate) fn evaluate_materialization_status(
    facts: MaterializationFacts,
    configured_strategy: MaterializationStrategy,
) -> MaterializationStatus {
    let observed = facts.observed_materialization();
    let materialization_exists = !matches!(observed, ObservedMaterialization::Missing);
    let materialization_valid = matches!(
        observed,
        ObservedMaterialization::ManagedSymlink | ObservedMaterialization::RegularCopy
    ) && !matches!(
        facts.copy_content,
        CopyContentState::Diverged
            | CopyContentState::Unreadable
            | CopyContentState::ComparisonFailed
    );

    let mut issues = Vec::new();
    if !materialization_exists {
        issues.push(issue(StatusIssueCode::MaterializationMissing));
    } else if !materialization_valid {
        issues.push(issue(match observed {
            ObservedMaterialization::UnsafeHardlink => StatusIssueCode::HardlinkUnsafe,
            ObservedMaterialization::Unreadable => StatusIssueCode::ContentUnreadable,
            _ => StatusIssueCode::MaterializationInvalid,
        }));
    }
    match facts.store_state {
        StoreState::Missing => issues.push(issue(StatusIssueCode::StoreMissing)),
        StoreState::Unreadable | StoreState::Unsupported => {
            issues.push(issue(StatusIssueCode::ContentUnreadable));
        }
        StoreState::Present => {}
    }
    match facts.copy_content {
        CopyContentState::Diverged => issues.push(issue(StatusIssueCode::ContentDiverged)),
        CopyContentState::Unreadable | CopyContentState::ComparisonFailed => {
            if !issues
                .iter()
                .any(|item| item.code == StatusIssueCode::ContentUnreadable)
            {
                issues.push(issue(StatusIssueCode::ContentUnreadable));
            }
        }
        CopyContentState::NotCompared | CopyContentState::Equal => {}
    }
    if facts.git_state != GitState::Untracked {
        issues.push(issue(StatusIssueCode::TrackedByGit));
    }
    if facts.exclude_state == ExcludeState::Missing {
        issues.push(issue(StatusIssueCode::MissingExclude));
    } else if facts.exclude_state == ExcludeState::QueryFailed {
        issues.push(issue(StatusIssueCode::MaterializationInvalid));
    }

    let has_error = !materialization_valid
        || !matches!(facts.store_state, StoreState::Present)
        || !matches!(facts.git_state, GitState::Untracked)
        || matches!(facts.exclude_state, ExcludeState::QueryFailed)
        || matches!(facts.copy_content, CopyContentState::Diverged | CopyContentState::Unreadable | CopyContentState::ComparisonFailed)
        // A regular copy without an exact exclude entry is Git-visible secret
        // content and therefore an error; a symlink retains legacy warning.
        || (facts.exclude_state == ExcludeState::Missing
            && observed == ObservedMaterialization::RegularCopy);
    let severity = if has_error {
        StatusSeverity::Error
    } else if facts.exclude_state == ExcludeState::Missing {
        StatusSeverity::Warning
    } else {
        StatusSeverity::Healthy
    };

    let actual_strategy = match observed {
        ObservedMaterialization::ManagedSymlink => Some(MaterializationStrategy::Symlink),
        ObservedMaterialization::RegularCopy => Some(MaterializationStrategy::Copy),
        _ => None,
    };
    let notes = (actual_strategy.is_some() && actual_strategy != Some(configured_strategy))
        .then_some(StatusNote {
            code: StatusNoteCode::StrategyMismatch,
        })
        .into_iter()
        .collect();

    MaterializationStatus {
        observed,
        materialization_exists,
        materialization_valid,
        severity,
        issues,
        notes,
    }
}

fn issue(code: StatusIssueCode) -> StatusIssue {
    StatusIssue { code }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::materialization::{MaterializationRelation, RepoEntryKind, StoreState};

    fn copy_facts() -> MaterializationFacts {
        MaterializationFacts {
            repo_entry: RepoEntryKind::RegularFile,
            relation: MaterializationRelation::IsolatedRegularCopy,
            copy_content: CopyContentState::Equal,
            store_state: StoreState::Present,
            git_state: GitState::Untracked,
            exclude_state: ExcludeState::Present,
        }
    }

    #[test]
    fn healthy_mixed_strategy_is_an_informational_note() {
        let result =
            evaluate_materialization_status(copy_facts(), MaterializationStrategy::Symlink);
        assert_eq!(result.severity, StatusSeverity::Healthy);
        assert_eq!(
            result.notes,
            vec![StatusNote {
                code: StatusNoteCode::StrategyMismatch
            }]
        );
    }

    #[test]
    fn copy_missing_exclude_is_an_error_but_symlink_is_a_warning() {
        let mut copy = copy_facts();
        copy.exclude_state = ExcludeState::Missing;
        assert_eq!(
            evaluate_materialization_status(copy, MaterializationStrategy::Copy).severity,
            StatusSeverity::Error
        );

        copy.repo_entry = RepoEntryKind::Symlink;
        copy.relation = MaterializationRelation::ManagedSymlink;
        copy.copy_content = CopyContentState::NotCompared;
        assert_eq!(
            evaluate_materialization_status(copy, MaterializationStrategy::Symlink).severity,
            StatusSeverity::Warning
        );
    }
}
