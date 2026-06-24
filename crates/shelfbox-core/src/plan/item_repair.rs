use crate::plan::repo_repair::RepoRepairSymlinkAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairOutcome {
    /// The symlink was recreated or relinked to the correct target.
    LinkRecreated,
    /// The item was already healthy; no action was taken.
    AlreadyHealthy,
    /// The store-side file is missing; cannot repair without data recovery.
    StoreMissing,
    /// The path is not recorded in the manifest.
    NotManaged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRepairReport {
    pub action: RepoRepairSymlinkAction,
    pub outcome: RepairOutcome,
    pub dry_run: bool,
}
