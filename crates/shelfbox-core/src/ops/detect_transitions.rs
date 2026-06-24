use crate::{
    config::Config,
    context::RepoContext,
    error::Result,
    store::{index, manifest, manifest::OwnershipState},
};

/// Summary of ownership state transitions detected by transition scans.
#[derive(Debug, Default)]
pub struct TransitionReport {
    /// Number of items transitioned from `Attached` to `Unreachable`.
    pub unreachable: usize,
    /// Repo IDs whose manifests were updated.
    pub affected_repos: Vec<String>,
}

impl TransitionReport {
    /// Returns `true` if no transitions were performed.
    pub fn is_empty(&self) -> bool {
        self.unreachable == 0
    }
}

/// Read-only scan: returns how many `Attached` items in OTHER repos would be
/// considered unreachable. Used by `repo status` to display a non-mutating
/// hint without writing anything.
pub fn scan(ctx: &RepoContext, config: &Config) -> Result<TransitionReport> {
    let mut report = TransitionReport::default();

    let idx = index::load(&config.store)?;

    for (repo_id, entry) in idx.iter() {
        if repo_id == ctx.repo_id {
            continue;
        }

        if entry.root.as_ref().is_some_and(|root| root.exists()) {
            continue;
        }

        let repo_store = config.store.join("repos").join(&entry.repo_store_dir);
        let mf = match manifest::load(&repo_store) {
            Ok(m) => m,
            Err(_) => continue,
        };

        for item in &mf.items {
            if item.ownership_state != OwnershipState::Attached {
                continue;
            }
            report.unreachable += 1;
            if !report.affected_repos.contains(&repo_id.to_owned()) {
                report.affected_repos.push(repo_id.to_owned());
            }
        }
    }

    Ok(report)
}

// ── Helper: OwnershipState must be Clone for the loop above ──────────────────
// Verify the domain type's clone requirement at compile time.
const _: fn() = || {
    fn _assert_clone<T: Clone>() {}
    _assert_clone::<OwnershipState>();
};

// ── Tests ─────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_report_is_empty() {
        let report = TransitionReport::default();
        assert!(report.is_empty());
    }

    #[test]
    fn non_empty_unreachable_not_empty() {
        let report = TransitionReport {
            unreachable: 1,
            affected_repos: vec!["A".into()],
        };
        assert!(!report.is_empty());
    }
}
