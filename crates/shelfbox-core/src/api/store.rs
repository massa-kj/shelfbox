use std::path::Path;

pub use crate::{
    config::Config,
    store::{
        index::{Index, RepoEntry},
        manifest::Manifest,
        meta::StoreMeta,
    },
};

pub use crate::ops::store_verify::{
    StoreVerifyIssue, StoreVerifyIssueCode, StoreVerifyReport, StoreVerifySeverity,
};
pub use crate::plan::{
    manifest_migration::{MigrationReport, MigrationSkip},
    store_gc::{GcCandidate, GcPlan, GcReport},
    store_rebuild_index::{RebuildIndexReport, RebuildIndexWarning},
};

use crate::{
    context,
    error::Result,
    ops::{gc, migrate_manifest, rebuild_index, store_verify},
    store::{index, manifest, meta},
};

pub fn load_config(store_override: Option<&Path>) -> Result<Config> {
    Config::load(store_override)
}

pub fn load_index(store_root: &Path) -> Result<Index> {
    index::load(store_root)
}

pub fn load_manifest(repo_store: &Path) -> Result<Manifest> {
    manifest::load(repo_store)
}

pub fn load_store_meta(store_root: &Path) -> Result<Option<StoreMeta>> {
    meta::load_store_meta(store_root)
}

/// Runs store-wide, read-only integrity verification.
pub fn verify(store_root: &Path) -> Result<StoreVerifyReport> {
    store_verify::verify(store_root)
}

pub fn gc_plan(store_root: &Path) -> Result<GcPlan> {
    gc::plan(store_root)
}

pub fn gc_run(store_root: &Path, dry_run: bool) -> Result<GcReport> {
    let _store_lock = (!dry_run)
        .then(|| context::acquire_store_write_access(store_root))
        .transpose()?;
    gc::run(store_root, dry_run)
}

pub fn rebuild_index(store_root: &Path, dry_run: bool) -> Result<RebuildIndexReport> {
    let _store_lock = (!dry_run)
        .then(|| context::acquire_store_write_access(store_root))
        .transpose()?;
    rebuild_index::run(store_root, dry_run)
}

pub fn migrate_manifests(store_root: &Path, dry_run: bool) -> Result<MigrationReport> {
    let _store_lock = (!dry_run)
        .then(|| context::acquire_store_write_access(store_root))
        .transpose()?;
    migrate_manifest::run(store_root, dry_run)
}
