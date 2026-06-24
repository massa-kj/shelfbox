use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemAddPlan {
    pub path: String,
    pub abs_path: PathBuf,
    pub store_path: PathBuf,
    pub store_path_relative: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemAddReport {
    pub plan: ItemAddPlan,
    pub dry_run: bool,
}
