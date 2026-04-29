use std::path::Path;

use crate::{
    context::{self, RepoContext},
    error::{AppError, Result},
    git,
    ignore::IgnoreBackend,
    link::LinkStrategy,
    store::manifest::{self, GitInfo, Item, ItemKind, LinkInfo, LinkType},
};

/// Shelves `abs_path` into the store, leaving a symlink in its place.
///
/// # Dry-run
/// When `dry_run` is `true`, prints what would happen without making changes.
///
/// # Errors
///
/// Returns an error if any pre-condition is violated (see [`crate::error::AppError`]
/// variants for the full list).
pub fn add(
    ctx: &mut RepoContext,
    abs_path: &Path,
    dry_run: bool,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<()> {
    // ── Path validation ──────────────────────────────────────────────────────
    // Must be within the repository root.
    let rel_path =
        abs_path
            .strip_prefix(&ctx.repo_root)
            .map_err(|_| AppError::PathOutsideRepo {
                path: abs_path.to_path_buf(),
            })?;
    let rel_str = rel_path.to_string_lossy().into_owned();

    // Must not be inside .git/.
    if rel_path.starts_with(".git") {
        return Err(AppError::PathInsideGitDir {
            path: abs_path.to_path_buf(),
        });
    }

    // Read symlink metadata so we can distinguish symlinks from regular entries
    // without following the link (also validates the path exists).
    let meta = abs_path
        .symlink_metadata()
        .map_err(|e| AppError::io(abs_path, e))?;

    // Must not already be a symlink.
    if meta.file_type().is_symlink() {
        return Err(AppError::PathIsSymlink {
            path: abs_path.to_path_buf(),
        });
    }

    // Must not be tracked by Git.
    if git::is_tracked(&ctx.repo_root, abs_path)? {
        return Err(AppError::PathIsTracked {
            path: abs_path.to_path_buf(),
        });
    }

    // Must not already be managed by repo-shelve.
    if ctx.manifest.contains(&rel_str) {
        return Err(AppError::AlreadyManaged {
            path: abs_path.to_path_buf(),
        });
    }

    // Store destination must not already be occupied.
    let store_path = ctx.store_path_for(&rel_str);
    if store_path.exists() {
        return Err(AppError::StoreConflict {
            store_path: store_path.clone(),
        });
    }

    let kind = if meta.is_dir() {
        ItemKind::Directory
    } else {
        ItemKind::File
    };

    // Store-relative path (relative to repo_store): "items/<rel>".
    let store_path_rel = format!("items/{rel_str}");

    // ── Dry-run ──────────────────────────────────────────────────────────────
    if dry_run {
        println!("[dry-run] shelve '{rel_str}'");
        println!(
            "  move    {} → {}",
            abs_path.display(),
            store_path.display()
        );
        println!(
            "  symlink {} → {}",
            abs_path.display(),
            store_path.display()
        );
        println!("  exclude {rel_str}");
        return Ok(());
    }

    // ── Execute ──────────────────────────────────────────────────────────────
    // Ensure the items sub-directory exists before the move.
    if let Some(parent) = store_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    // Move the file/directory into the store.
    std::fs::rename(abs_path, &store_path).map_err(|e| AppError::io(abs_path, e))?;

    // Create the symlink; roll back the move on failure.
    if let Err(e) = link.create(&store_path, abs_path) {
        let _ = std::fs::rename(&store_path, abs_path);
        return Err(e);
    }

    // Update the remote URL in the manifest on first shelve if not yet recorded.
    if ctx.manifest.repo.remote.is_none() {
        ctx.manifest.repo.remote = git::get_remote_url(&ctx.repo_root)?;
    }

    // Record the item in the manifest.
    let now = context::now_iso8601();
    let item = Item {
        path: rel_str.clone(),
        store_path: store_path_rel,
        kind,
        link: LinkInfo {
            link_type: LinkType::Symlink,
        },
        git: GitInfo { was_tracked: false },
        created_at: now.clone(),
        updated_at: now,
    };
    ctx.manifest.add(item);
    manifest::save(&ctx.repo_store, &ctx.manifest)?;

    // Add the path to .git/info/exclude.
    ignore.add_entries(&ctx.repo_root, &[&rel_str])?;

    Ok(())
}
