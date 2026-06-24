use std::path::{Path, PathBuf};

use ulid::Ulid;

use super::path::{normalize_repo_relative, repo_relative_path, repo_relative_string};
use crate::{
    context::{self, RepoContext},
    error::{AppError, Result},
    git,
    ignore::IgnoreBackend,
    link::LinkStrategy,
    plan::item_add::{ItemAddPlan, ItemAddReport},
    policy::item_validation::{self, DirectoryCandidateDecision},
    store::manifest::{self, Item, OwnershipState},
};

fn update_identity_hints(ctx: &mut RepoContext) {
    if let Some(name) = ctx.repo_root.file_name().and_then(|n| n.to_str()) {
        ctx.manifest.add_repo_name_hint(name);
    }

    if let Ok(Some(remote_url)) = git::remote_url(&ctx.repo_root) {
        if let Some(remote_hint) = git::normalize_remote_hint(&remote_url) {
            ctx.manifest.add_remote_hint(&remote_hint);
        }
    }
}

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
    add_report(ctx, abs_path, dry_run, link, ignore).map(|_| ())
}

pub(crate) fn add_report(
    ctx: &mut RepoContext,
    abs_path: &Path,
    dry_run: bool,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<ItemAddReport> {
    let plan = build_add_plan(ctx, abs_path)?;

    if !dry_run {
        execute_add_plan(ctx, &plan, link, ignore)?;
    }

    Ok(ItemAddReport { plan, dry_run })
}

fn build_add_plan(ctx: &RepoContext, abs_path: &Path) -> Result<ItemAddPlan> {
    // ── Path validation ──────────────────────────────────────────────────────
    // Must be within the repository root.
    let rel_path = repo_relative_path(&ctx.repo_root, abs_path)?;
    let rel_str = repo_relative_string(&ctx.repo_root, abs_path)?;

    item_validation::validate_add_location(&rel_path, abs_path)?;

    // Read symlink metadata so we can distinguish symlinks from regular entries
    // without following the link (also validates the path exists).
    let meta = abs_path
        .symlink_metadata()
        .map_err(|e| AppError::io(abs_path, e))?;
    let kind = item_validation::add_entry_kind_from_meta(&meta);

    item_validation::validate_add_entry_kind(abs_path, kind)?;

    // Must not be tracked by Git.
    item_validation::validate_add_git_state(abs_path, git::is_tracked(&ctx.repo_root, abs_path)?)?;

    // Must not already be managed by shelfbox.
    item_validation::validate_add_manifest_state(abs_path, ctx.manifest.contains(&rel_str))?;

    // Store destination must not already be occupied.
    let store_path = ctx.store_path_for(&rel_str);
    item_validation::validate_add_store_destination(&store_path, store_path.exists())?;

    // Store-relative path (relative to repo_store): "items/<rel>".
    let store_path_rel = item_validation::store_item_path_for_repo_path(&rel_str);

    Ok(ItemAddPlan {
        path: rel_str,
        abs_path: abs_path.to_path_buf(),
        store_path,
        store_path_relative: store_path_rel,
    })
}

fn execute_add_plan(
    ctx: &mut RepoContext,
    plan: &ItemAddPlan,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<()> {
    // ── Execute ──────────────────────────────────────────────────────────────
    // Ensure the items sub-directory exists before the move.
    if let Some(parent) = plan.store_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    // Move the file/directory into the store.
    std::fs::rename(&plan.abs_path, &plan.store_path)
        .map_err(|e| AppError::io(&plan.abs_path, e))?;

    // Create the symlink; roll back the move on failure.
    if let Err(e) = link.create(&plan.store_path, &plan.abs_path) {
        let _ = std::fs::rename(&plan.store_path, &plan.abs_path);
        return Err(e);
    }

    // Record the item in the manifest.
    let now = context::now_iso8601();
    update_identity_hints(ctx);
    let item = Item {
        item_id: Ulid::new().to_string(),
        origin_repo_id: ctx.repo_id.clone(),
        path: plan.path.clone(),
        store_path: plan.store_path_relative.clone(),
        ownership_state: OwnershipState::Attached,
        created_at: now.clone(),
        updated_at: now,
    };
    ctx.manifest.add(item);
    manifest::save(&ctx.repo_store, &ctx.manifest)?;

    // Add the path to .git/info/exclude.
    ignore.add_entries(&ctx.repo_root, &[&plan.path])?;

    Ok(())
}

// ── Directory namespace shelving ───────────────────────────────────────────────

/// Why a candidate file was skipped during a directory add.
#[derive(Debug)]
pub enum SkipReason {
    /// Already recorded in the shelfbox manifest.
    AlreadyManaged,
    /// Tracked by git; shelving is refused.
    GitTracked,
    /// Already a symlink; shelving symlinks is not supported.
    IsSymlink,
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkipReason::AlreadyManaged => write!(f, "already managed by shelfbox"),
            SkipReason::GitTracked => write!(f, "tracked by git"),
            SkipReason::IsSymlink => write!(f, "is a symlink"),
        }
    }
}

/// Outcome for a single file processed during [`add_directory`].
#[derive(Debug)]
pub enum DirItemOutcome {
    /// File was successfully shelved.
    Added,
    /// File would be shelved (dry-run mode).
    WouldAdd,
    /// File was skipped with a benign reason.
    Skipped(SkipReason),
    /// A nested git repository was found and its contents were excluded.
    NestedGitRepo,
    /// Shelving failed with an error.
    Failed(String),
}

/// Summary of a [`add_directory`] operation.
#[derive(Debug)]
pub struct DirectoryAddResult {
    /// Directory path that was processed (repo-relative, ends with `/`).
    pub ns_path: String,
    /// Per-file outcomes in the order they were processed.
    pub results: Vec<(String, DirItemOutcome)>,
    /// Always false in v0.7.0; namespaces are UI-only and not persisted.
    pub namespace_created: bool,
}

/// Shelves all eligible files under `abs_dir`.
///
/// Each eligible file is moved to the store and replaced with a symlink.
///
/// # Eligibility rules
///
/// A file is eligible if it is:
/// - not already managed by shelfbox,
/// - not tracked by git,
/// - not a symlink.
///
/// Nested git repositories inside `abs_dir` are reported as
/// [`DirItemOutcome::NestedGitRepo`] and their contents are excluded entirely.
/// Partial success is allowed.
///
/// # Dry-run
/// When `dry_run` is `true`, no filesystem changes are made.
pub fn add_directory(
    ctx: &mut RepoContext,
    abs_dir: &Path,
    dry_run: bool,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<DirectoryAddResult> {
    // ── Validate the directory ───────────────────────────────────────────────
    let rel_dir = repo_relative_path(&ctx.repo_root, abs_dir)?;
    let rel_str = repo_relative_string(&ctx.repo_root, abs_dir)?;

    item_validation::validate_add_location(&rel_dir, abs_dir)?;

    // Namespace path always ends with "/" for unambiguous prefix matching.
    let ns_path = format!("{rel_str}/");

    // ── Collect candidates ───────────────────────────────────────────────────
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut nested_repos: Vec<PathBuf> = Vec::new();
    collect_dir_candidates(abs_dir, &mut candidates, &mut nested_repos)
        .map_err(|e| AppError::io(abs_dir, e))?;

    // Pre-fetch git-tracked paths in the directory (one subprocess instead of N).
    let tracked = git::tracked_files_in_dir(&ctx.repo_root, abs_dir).unwrap_or_default();

    let mut results: Vec<(String, DirItemOutcome)> = Vec::new();

    // Report nested git repos as non-fatal blocking entries.
    for nested in &nested_repos {
        let rel_buf =
            repo_relative_path(&ctx.repo_root, nested).unwrap_or_else(|_| nested.to_path_buf());
        let rel = rel_buf.as_path();
        results.push((normalize_repo_relative(rel), DirItemOutcome::NestedGitRepo));
    }

    // ── Process each candidate ───────────────────────────────────────────────
    let mut to_shelve: Vec<(String, PathBuf, String)> = Vec::new(); // (rel, abs, store_path_rel)

    for candidate in candidates {
        let rel_cand = repo_relative_path(&ctx.repo_root, &candidate)?;
        let rel_cand_str = normalize_repo_relative(&rel_cand);

        let meta = match candidate.symlink_metadata() {
            Ok(m) => m,
            Err(e) => {
                results.push((
                    rel_cand_str,
                    DirItemOutcome::Failed(format!("failed to stat: {e}")),
                ));
                continue;
            }
        };
        let store_path = ctx.store_path_for(&rel_cand_str);
        let decision = item_validation::classify_directory_candidate(
            ctx.manifest.contains(&rel_cand_str),
            meta.file_type().is_symlink(),
            tracked.contains(&rel_cand_str),
            store_path.exists(),
        );
        match decision {
            DirectoryCandidateDecision::Add => {}
            DirectoryCandidateDecision::SkipAlreadyManaged => {
                results.push((
                    rel_cand_str,
                    DirItemOutcome::Skipped(SkipReason::AlreadyManaged),
                ));
                continue;
            }
            DirectoryCandidateDecision::SkipGitTracked => {
                results.push((
                    rel_cand_str,
                    DirItemOutcome::Skipped(SkipReason::GitTracked),
                ));
                continue;
            }
            DirectoryCandidateDecision::SkipSymlink => {
                results.push((rel_cand_str, DirItemOutcome::Skipped(SkipReason::IsSymlink)));
                continue;
            }
            DirectoryCandidateDecision::StoreConflict => {
                results.push((
                    rel_cand_str,
                    DirItemOutcome::Failed(item_validation::conflict_message(store_path)),
                ));
                continue;
            }
        }

        let store_path_rel = item_validation::store_item_path_for_repo_path(&rel_cand_str);
        to_shelve.push((rel_cand_str, candidate, store_path_rel));
    }

    // ── Dry-run ──────────────────────────────────────────────────────────────
    if dry_run {
        for (rel, _, _) in &to_shelve {
            results.push((rel.clone(), DirItemOutcome::WouldAdd));
        }
        return Ok(DirectoryAddResult {
            ns_path,
            results,
            namespace_created: false,
        });
    }

    // ── Execute shelving ─────────────────────────────────────────────────────
    let now = context::now_iso8601();
    update_identity_hints(ctx);
    let mut added_paths: Vec<String> = Vec::new();

    for (rel_cand_str, abs_cand, store_path_rel) in to_shelve {
        let store_path = ctx.store_path_for(&rel_cand_str);

        if let Some(parent) = store_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                results.push((
                    rel_cand_str,
                    DirItemOutcome::Failed(format!("create store dir: {e}")),
                ));
                continue;
            }
        }

        if let Err(e) = std::fs::rename(&abs_cand, &store_path) {
            results.push((
                rel_cand_str,
                DirItemOutcome::Failed(format!("move to store: {e}")),
            ));
            continue;
        }

        if let Err(e) = link.create(&store_path, &abs_cand) {
            // Roll back the move to avoid data loss.
            let _ = std::fs::rename(&store_path, &abs_cand);
            results.push((
                rel_cand_str,
                DirItemOutcome::Failed(format!("create symlink: {e}")),
            ));
            continue;
        }

        let item = Item {
            item_id: Ulid::new().to_string(),
            origin_repo_id: ctx.repo_id.clone(),
            path: rel_cand_str.clone(),
            store_path: store_path_rel,
            ownership_state: OwnershipState::Attached,
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        ctx.manifest.add(item);
        added_paths.push(rel_cand_str.clone());
        results.push((rel_cand_str, DirItemOutcome::Added));
    }

    if !added_paths.is_empty() {
        manifest::save(&ctx.repo_store, &ctx.manifest)?;

        let refs: Vec<&str> = added_paths.iter().map(String::as_str).collect();
        ignore.add_entries(&ctx.repo_root, &refs)?;
    }

    Ok(DirectoryAddResult {
        ns_path,
        results,
        namespace_created: false,
    })
}

/// Recursively collects file candidates from `dir`.
///
/// Directories that contain a `.git` entry are recorded in `nested_repos`
/// and not descended into.  Symlinks to directories are treated as file
/// candidates (not traversed).
fn collect_dir_candidates(
    dir: &Path,
    candidates: &mut Vec<PathBuf>,
    nested_repos: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // Use file_type() which does NOT follow symlinks.
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            // Refuse to cross a nested git repository boundary.
            if path.join(".git").exists() {
                nested_repos.push(path);
            } else {
                collect_dir_candidates(&path, candidates, nested_repos)?;
            }
        } else {
            // Regular files and symlinks are both collected here.
            // The symlink check is done in add_directory() to report them
            // as SkipReason::IsSymlink rather than silently ignoring.
            candidates.push(path);
        }
    }
    Ok(())
}
