use serde::Serialize;

use crate::{
    context::RepoContext, error::Result, git, ignore::IgnoreBackend, link::LinkStrategy,
    store::manifest::Item,
};

/// Health status for a single shelved item.
#[derive(Debug, Serialize)]
pub struct ItemStatus {
    /// Repo-relative path of the item.
    pub path: String,
    /// `true` if a filesystem entry exists at the repo-side path
    /// (including dangling symlinks).
    pub link_exists: bool,
    /// `true` if the repo-side path is a managed symlink pointing into the store.
    pub link_valid: bool,
    /// `true` if the store-side path exists on disk.
    pub store_exists: bool,
    /// `true` if the path appears in `.git/info/exclude`.
    pub in_exclude: bool,
    /// `true` when the path is NOT tracked by Git (expected for shelved items).
    pub not_tracked: bool,
    /// `true` when every other field is `true`.
    pub ok: bool,
}

/// Returns the health status of every item currently in the manifest.
pub fn status(
    ctx: &RepoContext,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<Vec<ItemStatus>> {
    ctx.manifest
        .items
        .iter()
        .map(|item| check_item(ctx, item, link, ignore))
        .collect()
}

fn check_item(
    ctx: &RepoContext,
    item: &Item,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<ItemStatus> {
    let abs_path = ctx.repo_root.join(&item.path);
    let store_path = ctx.repo_store.join(&item.store_path);

    // Does a symlink exist at the repo-side path (via the link strategy)?
    let link_exists = link.is_link(&abs_path);
    // Is it specifically a managed symlink pointing into the store?
    let link_valid = link.is_managed_link(&abs_path, &ctx.config.store);
    // Does the store-side copy exist?
    let store_exists = store_path.exists();
    // Is the path listed in .git/info/exclude?
    let in_exclude = ignore.has_entry(&ctx.repo_root, &item.path)?;
    // Is the path not tracked by Git (i.e. not accidentally staged as the symlink)?
    let not_tracked = !git::is_tracked(&ctx.repo_root, &abs_path)?;

    let ok = link_exists && link_valid && store_exists && in_exclude && not_tracked;

    Ok(ItemStatus {
        path: item.path.clone(),
        link_exists,
        link_valid,
        store_exists,
        in_exclude,
        not_tracked,
        ok,
    })
}
