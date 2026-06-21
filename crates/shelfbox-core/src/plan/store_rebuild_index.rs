#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebuildIndexWarning {
    pub repo_store_dir: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebuildIndexReport {
    pub repositories: usize,
    pub warnings: Vec<RebuildIndexWarning>,
    pub dry_run: bool,
}
