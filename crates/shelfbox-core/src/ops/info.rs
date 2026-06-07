use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{context::RepoContext, error::Result, ignore::IgnoreBackend, link::LinkStrategy};

use super::path::repo_relative_string;

/// Diagnostic metadata for a single shelved item.
///
/// Returned by [`info`] and intended for use as a debugging / scripting API.
/// Every field is always populated regardless of the item's current state,
/// making this the canonical source for "why does this item look broken?"
/// diagnostics.
#[derive(Debug, Serialize)]
pub struct ItemInfo {
    /// Repo-relative path of the item (forward slashes, no leading `/`).
    pub path: String,

    /// Absolute path to the repository root.
    pub repo_root: PathBuf,

    /// Absolute path to the store-side file.
    /// `None` if the item is not in the manifest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store_path: Option<PathBuf>,

    /// Target returned by `readlink(2)` at the repo path.
    /// `None` if no symlink exists at that path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_target: Option<PathBuf>,

    /// `true` if a symlink exists at the repo path *and* its target matches
    /// `store_path` exactly.
    pub symlink_ok: bool,

    /// `true` if the item appears in the manifest.
    pub tracked: bool,

    /// `true` if the path appears in `.git/info/exclude`.
    pub in_exclude: bool,
}

/// Returns diagnostic metadata for the item at `abs_path`.
///
/// `abs_path` must be absolute and located under `ctx.repo_root`.
/// Returns [`AppError::PathOutsideRepo`] if it is not.
///
/// The `link` argument is accepted for API symmetry with other ops but is not
/// used; symlink validity is determined by comparing `readlink` output against
/// the manifest's `store_path`.
pub fn info(
    ctx: &RepoContext,
    abs_path: &Path,
    _link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<ItemInfo> {
    // Convert abs_path → repo-relative string (forward slashes).
    let rel_str = repo_relative_string(&ctx.repo_root, abs_path)?;

    // Look up the manifest entry.
    let manifest_item = ctx.manifest.items.iter().find(|item| item.path == rel_str);

    let tracked = manifest_item.is_some();

    // Resolve the absolute store path from the manifest entry.
    let store_path = manifest_item.map(|item| ctx.repo_store.join(&item.store_path));

    // Read the symlink target at the repo path (None if no symlink).
    let link_target = fs::read_link(abs_path).ok();

    // A symlink is healthy when it points exactly at the expected store path.
    let symlink_ok = match (&link_target, &store_path) {
        (Some(target), Some(expected)) => target == expected,
        _ => false,
    };

    // Check .git/info/exclude membership.
    let in_exclude = ignore.has_entry(&ctx.repo_root, &rel_str)?;

    Ok(ItemInfo {
        path: rel_str,
        repo_root: ctx.repo_root.clone(),
        store_path,
        link_target,
        symlink_ok,
        tracked,
        in_exclude,
    })
}
