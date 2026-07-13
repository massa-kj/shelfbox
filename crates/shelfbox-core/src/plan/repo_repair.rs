use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoRepairSymlinkAction {
    /// Legacy symlink-specific create action retained for source-compatible
    /// reports. New callers should treat this as a materialization action.
    Recreate {
        path: String,
        abs_path: PathBuf,
        store_path: PathBuf,
    },
    /// Creates a missing or force-replaceable regular Copy materialization.
    /// The operation executes this only through `Materializer` and an
    /// artifact journal; this plan never exposes a temp path.
    CreateCopy {
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
    /// A regular Copy is valid but does not equal its store source. Repair is
    /// observational here: it must not overwrite user content.
    CopyDiverged {
        path: String,
    },
    /// Detached items are intentionally excluded from materialization repair.
    DetachedDisabled {
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
            | Self::CreateCopy { path, .. }
            | Self::AlreadyHealthy { path }
            | Self::StoreMissing { path }
            | Self::CopyDiverged { path }
            | Self::DetachedDisabled { path }
            | Self::NotManaged { path }
            | Self::Failed { path, .. } => path,
        }
    }
}

/// Strategy-neutral name for new integrations. The legacy public type keeps
/// its original spelling until the explicit v2 API boundary.
pub type RepoRepairAction = RepoRepairSymlinkAction;

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
                | RepoRepairSymlinkAction::CreateCopy { .. }
                | RepoRepairSymlinkAction::AlreadyHealthy { .. } => None,
                RepoRepairSymlinkAction::CopyDiverged { .. }
                | RepoRepairSymlinkAction::DetachedDisabled { .. } => None,
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
