use crate::{
    config::Config,
    context::{self, RepoContext},
    error::Result,
    store::{index, manifest, manifest::OwnershipState},
};

/// Summary of ownership state transitions performed by [`run`].
#[derive(Debug, Default)]
pub struct TransitionReport {
    /// Number of items transitioned from `Attached` to `Stale`.
    pub stale: usize,
    /// Number of items transitioned from `Attached` to `Unreachable`.
    pub unreachable: usize,
    /// Repo IDs whose manifests were updated.
    pub affected_repos: Vec<String>,
}

impl TransitionReport {
    /// Returns `true` if no transitions were performed.
    pub fn is_empty(&self) -> bool {
        self.stale == 0 && self.unreachable == 0
    }
}

/// Scans all other repos in the store index and automatically transitions
/// `Attached` items to `Stale` or `Unreachable` as appropriate.
///
/// # Detection rules
///
/// For each repo entry in the index whose ID differs from `ctx.repo_id`:
///
/// * **`Attached → Stale`**: `entry.git_common_dir == ctx.git_common_dir`.
///   The current repo has superseded the old one (reclone or repo move). Items
///   that were `Attached` in the old repo are now stale — reclaimable via
///   `repo adopt`.
///
/// * **`Attached → Unreachable`**: `entry.root` no longer exists on disk (and
///   the above stale condition does not apply).  The repo has been deleted or
///   moved without re-registering.
///
/// # Guard
///
/// Only `Attached` items are candidates for automatic transition.  Items
/// already in `Detached`, `Stale`, `Unreachable`, `Adopted`, or `Orphaned`
/// state are left unchanged.  This prevents re-transitioning already-resolved
/// items if, for example, an index corruption creates a duplicate
/// `git_common_dir` entry for an already-adopted repo.
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

        // Determine which transition applies to this repo, if any.
        let target_state = if entry.git_common_dir == ctx.git_common_dir {
            // Same logical repo (same git_common_dir): current identity has
            // superseded this older ULID → items become Stale.
            OwnershipState::Stale
        } else if !entry.root.exists() {
            // Repo root path is gone: items become Unreachable.
            OwnershipState::Unreachable
        } else {
            continue;
        };

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

            match target_state {
                OwnershipState::Stale => report.stale += 1,
                OwnershipState::Unreachable => report.unreachable += 1,
                _ => {}
            }
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

        let target_state = if entry.git_common_dir == ctx.git_common_dir {
            OwnershipState::Stale
        } else if !entry.root.exists() {
            OwnershipState::Unreachable
        } else {
            continue;
        };

        let repo_store = config.store.join("repos").join(&entry.store_dir);
        let mf = match manifest::load(&repo_store) {
            Ok(m) => m,
            Err(_) => continue,
        };

        for item in &mf.items {
            if item.ownership_state != OwnershipState::Attached {
                continue;
            }
            match target_state {
                OwnershipState::Stale => report.stale += 1,
                OwnershipState::Unreachable => report.unreachable += 1,
                _ => {}
            }
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
    fn non_empty_stale_not_empty() {
        let report = TransitionReport {
            stale: 1,
            unreachable: 0,
            affected_repos: vec!["A".into()],
        };
        assert!(!report.is_empty());
    }
}
