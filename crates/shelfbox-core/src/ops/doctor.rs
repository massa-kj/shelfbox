use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{
    context::RepoContext, error::Result, ignore::IgnoreBackend, link::LinkStrategy, store::index,
};

use super::status::{self, ItemStatus};

/// Comprehensive health report produced by the `doctor` command.
///
/// This is a read-only operation.  A `--fix` mode is deferred to a later
/// milestone.
#[derive(Debug, Serialize)]
pub struct DoctorReport {
    /// Per-item health status (covers every item in the manifest).
    pub items: Vec<ItemStatus>,
    /// Store-side paths that exist on disk but are not referenced in the manifest.
    pub orphan_store_items: Vec<String>,
    /// `true` when the repo root recorded in the index matches `ctx.repo_root`.
    pub repo_root_matches_index: bool,
}

/// Runs all health checks and returns a [`DoctorReport`].
pub fn doctor(
    ctx: &RepoContext,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<DoctorReport> {
    let items = status::status(ctx, link, ignore)?;
    let orphan_store_items = collect_orphan_store_items(ctx);
    let repo_root_matches_index = check_repo_root_in_index(ctx)?;
    Ok(DoctorReport {
        items,
        orphan_store_items,
        repo_root_matches_index,
    })
}

/// Checks whether the repo root stored in the global index matches
/// [`ctx.repo_root`].
///
/// A mismatch indicates that the repository was moved or cloned to a different
/// path on this machine since it was first shelved.
fn check_repo_root_in_index(ctx: &RepoContext) -> Result<bool> {
    let idx = index::load(&ctx.config.store)?;
    Ok(idx
        .get(&ctx.repo_id)
        .map(|e| e.root == ctx.repo_root)
        .unwrap_or(false))
}

/// Walks the `items/` subtree of the repo store and collects any path that is
/// not referenced by the manifest.
fn collect_orphan_store_items(ctx: &RepoContext) -> Vec<String> {
    let items_dir = ctx.items_dir();
    if !items_dir.exists() {
        return Vec::new();
    }

    // Absolute paths for every item currently owned by the manifest.
    let managed: HashSet<PathBuf> = ctx
        .manifest
        .items
        .iter()
        .map(|i| ctx.repo_store.join(&i.store_path))
        .collect();

    let mut orphans = Vec::new();
    walk_for_orphans(&items_dir, &items_dir, &managed, &mut orphans);
    orphans
}

/// Recursively walks `dir`, collecting entries that are neither in `managed`
/// nor an ancestor directory of a managed path.
fn walk_for_orphans(
    dir: &Path,
    items_root: &Path,
    managed: &HashSet<PathBuf>,
    orphans: &mut Vec<String>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();

        if managed.contains(&path) {
            // Known managed item; do not recurse into managed directories.
            continue;
        }

        // Is this an intermediate directory created to hold managed items?
        let is_ancestor = managed.iter().any(|m| m.starts_with(&path));
        if is_ancestor {
            if path.is_dir() {
                walk_for_orphans(&path, items_root, managed, orphans);
            }
        } else {
            // Neither managed nor a parent of a managed item → orphan.
            if let Ok(rel) = path.strip_prefix(items_root) {
                orphans.push(rel.to_string_lossy().into_owned());
            }
            // Report orphan directories at their root; do not recurse into them.
        }
    }
}
