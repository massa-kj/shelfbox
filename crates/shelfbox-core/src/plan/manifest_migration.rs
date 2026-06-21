use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationSkip {
    pub repo_store_dir: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationReport {
    pub target_version: u32,
    pub converted: BTreeMap<u32, usize>,
    pub unchanged: BTreeMap<u32, usize>,
    pub skipped: Vec<MigrationSkip>,
    pub stale_to_unreachable: usize,
    pub adopted_to_detached: usize,
    pub namespace_entries_dropped: usize,
    pub dry_run: bool,
}

impl MigrationReport {
    pub fn converted_total(&self) -> usize {
        self.converted.values().sum()
    }

    pub fn unchanged_total(&self) -> usize {
        self.unchanged.values().sum()
    }
}
