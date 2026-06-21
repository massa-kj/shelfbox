use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcCandidate {
    pub repo_id: String,
    pub repo_store_dir: String,
    pub item_id: String,
    pub path: String,
    pub store_path: String,
    pub absolute_store_path: PathBuf,
    pub size_bytes: u64,
    pub store_exists: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GcPlan {
    pub candidates: Vec<GcCandidate>,
    pub protected_attached: usize,
    pub protected_detached: usize,
    pub protected_unreachable: usize,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GcReport {
    pub candidates: Vec<GcCandidate>,
    pub deleted_items: usize,
    pub missing_items: usize,
    pub bytes_reclaimed: u64,
    pub manifests_updated: usize,
    pub dry_run: bool,
}
