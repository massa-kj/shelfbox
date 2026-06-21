use std::path::Path;

pub use crate::ops::{
    gc::{GcCandidate, GcPlan, GcReport},
    migrate_manifest::{MigrationReport, MigrationSkip},
    rebuild_index::{RebuildIndexReport, RebuildIndexWarning},
};

use crate::{
    error::Result,
    ops::{gc, migrate_manifest, rebuild_index},
};

pub fn gc_plan(store_root: &Path) -> Result<GcPlan> {
    gc::plan(store_root)
}

pub fn gc_run(store_root: &Path, dry_run: bool) -> Result<GcReport> {
    gc::run(store_root, dry_run)
}

pub fn rebuild_index(store_root: &Path, dry_run: bool) -> Result<RebuildIndexReport> {
    rebuild_index::run(store_root, dry_run)
}

pub fn migrate_manifests(store_root: &Path, dry_run: bool) -> Result<MigrationReport> {
    migrate_manifest::run(store_root, dry_run)
}
