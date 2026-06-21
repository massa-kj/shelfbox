use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclaimCandidate {
    pub repo_store_dir: String,
    pub repo_id: String,
    pub score: i32,
    pub reasons: Vec<String>,
    pub item_count: usize,
    pub state: CandidateState,
    pub remote_hints: Vec<String>,
    pub last_attached_at: Option<String>,
    pub repo_name_hints: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateState {
    Unreachable,
    Detached,
    AttachedElsewhere,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclaimPlan {
    pub repo_id: String,
    pub repo_store_dir: String,
    pub repo_store: PathBuf,
    pub current_root: PathBuf,
    pub current_git_dir: PathBuf,
    pub current_git_common_dir: PathBuf,
    pub removed_association_ids: Vec<String>,
    pub repo_name_hint: Option<String>,
    pub remote_hint: Option<String>,
    pub attached_at: String,
    pub last_seen_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclaimOutcome {
    pub repo_id: String,
    pub repo_store_dir: String,
}
