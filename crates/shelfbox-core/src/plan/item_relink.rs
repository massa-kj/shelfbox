use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelinkOutcome {
    /// Symlink was recreated and ownership_state transitioned to `Attached`.
    Relinked,
    /// Symlink already pointed to the correct store path; only state was updated.
    StateUpdated,
    /// Dry-run: item would be relinked.
    WouldRelink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRelinkPlan {
    pub path: String,
    pub abs_path: PathBuf,
    pub store_path: PathBuf,
    pub symlink_ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRelinkReport {
    pub plan: ItemRelinkPlan,
    pub outcome: RelinkOutcome,
    pub dry_run: bool,
}
