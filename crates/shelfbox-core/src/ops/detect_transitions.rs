use crate::{
    config::Config,
    context::{self, RepoContext},
    error::Result,
    store::{index, manifest, manifest::OwnershipState},
};

/// Summary of ownership state transitions performed by [`run`].
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

/// Scans all other repos in the store index and automatically transitions
/// `Attached` items to `Unreachable` when their local root is gone.
///
/// # Detection rules
///
/// For each repo entry in the index whose ID differs from `ctx.repo_id`:
///
/// * **`Attached → Unreachable`**: `entry.root` no longer exists on disk (and
///   the repo has been deleted or moved without re-registering.
///
/// # Guard
///
/// Only `Attached` items are candidates for automatic transition.  Items
/// already in `Detached`, `Unreachable`, or `Orphaned`
/// state are left unchanged.  This prevents re-transitioning already-resolved
/// items if, for example, an index corruption creates a duplicate
/// `git_common_dir` entry for an already-associated repo.
///
/// # Write behaviour
///
/// This function writes to manifests of OTHER repos in the store.  The caller
/// is responsible for holding the store write lock (i.e. `ctx` must have been
/// built with `write = true`).
pub fn run(ctx: &RepoContext, config: &Config) -> Result<TransitionReport> {
    let mut report = TransitionReport::default();
    let now = context::now_iso8601();

    let idx = index::load(&config.store)?;

    for (repo_id, entry) in idx.iter() {
        // Skip the current repo.
        if repo_id == ctx.repo_id {
            continue;
        }

        let repo_store = config.store.join("repos").join(&entry.store_dir);

        if entry.root.exists() {
            continue;
        }
        let target_state = OwnershipState::Unreachable;

        // Load the repo's manifest; skip gracefully if missing or unreadable.
        let mut mf = match manifest::load(&repo_store) {
            Ok(m) => m,
            Err(_) => continue,
        };

        // Transition only Attached items — all other states are immutable
        // under automatic detection (spec §6.1, P4.1 constraint).
        let mut changed = false;
        for item in mf.items.iter_mut() {
            if item.ownership_state != OwnershipState::Attached {
                continue;
            }
            item.ownership_state = target_state;
            item.updated_at = now.clone();
            changed = true;

            report.unreachable += 1;
        }

        if changed {
            manifest::save(&repo_store, &mf)?;
            report.affected_repos.push(repo_id.to_owned());
        }
    }

    Ok(report)
}

/// Read-only scan: returns how many `Attached` items in OTHER repos would be
/// transitioned by [`run`].  Used by `repo status` to display a non-mutating
/// hint without writing anything.
pub fn scan(ctx: &RepoContext, config: &Config) -> Result<TransitionReport> {
    let mut report = TransitionReport::default();

    let idx = index::load(&config.store)?;

    for (repo_id, entry) in idx.iter() {
        if repo_id == ctx.repo_id {
            continue;
        }

        if entry.root.exists() {
            continue;
        }

        let repo_store = config.store.join("repos").join(&entry.store_dir);
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
// OwnershipState derives Clone in manifest.rs; verify here at compile time.
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
