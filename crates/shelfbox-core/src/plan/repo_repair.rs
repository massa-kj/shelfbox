use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoRepairSymlinkAction {
    Recreate {
        path: String,
        abs_path: PathBuf,
        store_path: PathBuf,
    },
    AlreadyHealthy {
        path: String,
    },
    StoreMissing {
        path: String,
    },
    NotManaged {
        path: String,
    },
    Failed {
        path: String,
        reason: String,
    },
}

impl RepoRepairSymlinkAction {
    pub fn path(&self) -> &str {
        match self {
            Self::Recreate { path, .. }
            | Self::AlreadyHealthy { path }
            | Self::StoreMissing { path }
            | Self::NotManaged { path }
            | Self::Failed { path, .. } => path,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RepoRepairPlan {
    pub symlink_actions: Vec<RepoRepairSymlinkAction>,
    pub exclude_paths: Vec<String>,
    pub exclude_updated: bool,
    pub index_updated: bool,
    pub hints_updated: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RepairRepoReport {
    pub plan: RepoRepairPlan,
    pub symlinks_repaired: usize,
    pub symlinks_already_healthy: usize,
    pub symlinks_failed: Vec<(String, String)>,
    pub exclude_updated: bool,
    pub index_updated: bool,
    pub hints_updated: bool,
}

impl RepairRepoReport {
    pub(crate) fn from_plan(plan: RepoRepairPlan) -> Self {
        let symlinks_repaired = plan
            .symlink_actions
            .iter()
            .filter(|action| matches!(action, RepoRepairSymlinkAction::Recreate { .. }))
            .count();
        let symlinks_already_healthy = plan
            .symlink_actions
            .iter()
            .filter(|action| matches!(action, RepoRepairSymlinkAction::AlreadyHealthy { .. }))
            .count();
        let symlinks_failed = plan
            .symlink_actions
            .iter()
            .filter_map(|action| match action {
                RepoRepairSymlinkAction::StoreMissing { path } => {
                    Some((path.clone(), "store item missing".to_string()))
                }
                RepoRepairSymlinkAction::NotManaged { path } => {
                    Some((path.clone(), "not managed".to_string()))
                }
                RepoRepairSymlinkAction::Failed { path, reason } => {
                    Some((path.clone(), reason.clone()))
                }
                RepoRepairSymlinkAction::Recreate { .. }
                | RepoRepairSymlinkAction::AlreadyHealthy { .. } => None,
            })
            .collect();

        Self {
            symlinks_repaired,
            symlinks_already_healthy,
            symlinks_failed,
            exclude_updated: plan.exclude_updated,
            index_updated: plan.index_updated,
            hints_updated: plan.hints_updated,
            plan,
        }
    }
}
