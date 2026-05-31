use std::path::Path;

use crate::{
    context::{self, RepoContext},
    error::{AppError, Result},
    link::LinkStrategy,
    store::{
        index,
        manifest::{self, Item, OwnershipState},
    },
};

// ── Public types ───────────────────────────────────────────────────────────────

/// The outcome for a single item processed by [`adopt`].
#[derive(Debug, PartialEq, Eq)]
pub enum AdoptOutcome {
    /// Item adopted (transfer): store file copied and symlink updated.
    /// Source item transitions to `Adopted`.
    Adopted,
    /// Item adopted (transfer) but the symlink could not be updated (non-fatal).
    AdoptedLinkFailed,
    /// Item reclaimed (same logical repo): store file copied and symlink updated.
    /// Source item transitions back to `Attached` (spec §6.1: `unreachable → attached`).
    Reclaimed,
    /// Item reclaimed but the symlink could not be updated (non-fatal).
    ReclaimedLinkFailed,
    /// Dry-run: item would be adopted or reclaimed.
    WouldAdopt,
    /// Item skipped: a matching path is already managed by the current repo.
    Conflict,
    /// Item skipped: store file is missing in the source repo.
    StoreMissing,
}

/// Per-item result for a [`adopt`] operation.
#[derive(Debug)]
pub struct AdoptedItem {
    pub path: String,
    pub item_id: String,
    pub outcome: AdoptOutcome,
}

/// Aggregate result returned by [`adopt`].
#[derive(Debug)]
pub struct AdoptResult {
    /// Repo ID items were adopted from.
    pub from_repo_id: String,
    /// Per-item outcomes, one per eligible candidate in the source manifest.
    pub items: Vec<AdoptedItem>,
}

impl AdoptResult {
    /// Number of items that were (or in dry-run, would be) adopted or reclaimed.
    pub fn adopted_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| {
                matches!(
                    i.outcome,
                    AdoptOutcome::Adopted
                        | AdoptOutcome::AdoptedLinkFailed
                        | AdoptOutcome::Reclaimed
                        | AdoptOutcome::ReclaimedLinkFailed
                        | AdoptOutcome::WouldAdopt
                )
            })
            .count()
    }
}

// ── Core operation ─────────────────────────────────────────────────────────────

/// Adopts all eligible items from `from_repo_id` into the current repository.
///
/// Eligible items are those in the source manifest with `ownership_state` of
/// `Attached`, `Stale`, or `Unreachable`.  For each eligible item the
/// operation:
///
/// 1. Copies the store file from `repos/<from>/items/` to `repos/<current>/items/`.
/// 2. Adds the item to the current manifest, preserving `item_id` and
///    `origin_repo_id` (both are immutable ownership identifiers).
/// 3. Sets `ownership_state = Adopted` on the source manifest entry.
/// 4. Recreates the symlink at `<repo_root>/<item_path>` pointing to the new
///    store location.
///
/// When `dry_run` is `true` no filesystem changes are made.
///
/// # Errors
///
/// - [`AppError::AdoptFromSelf`] — `from_repo_id` equals `ctx.repo_id`.
/// - [`AppError::AdoptSourceNotFound`] — no index entry for `from_repo_id`.
pub fn adopt(
    ctx: &mut RepoContext,
    from_repo_id: &str,
    dry_run: bool,
    link: &dyn LinkStrategy,
) -> Result<AdoptResult> {
    if from_repo_id == ctx.repo_id {
        return Err(AppError::AdoptFromSelf {
            repo_id: from_repo_id.to_owned(),
        });
    }

    // Resolve source store directory from the index.
    let idx = index::load(&ctx.config.store)?;
    let src_entry = idx
        .get(from_repo_id)
        .ok_or_else(|| AppError::AdoptSourceNotFound {
            repo_id: from_repo_id.to_owned(),
        })?;
    let src_store = ctx.config.store.join("repos").join(&src_entry.store_dir);
    // Clone before `src_entry` borrow ends; needed for reclaim heuristic below.
    let src_git_common_dir = src_entry.git_common_dir.clone();

    let mut src_manifest = manifest::load(&src_store)?;

    const ELIGIBLE: &[OwnershipState] = &[
        OwnershipState::Attached,
        OwnershipState::Stale,
        OwnershipState::Unreachable,
    ];

    let candidates: Vec<Item> = src_manifest
        .items
        .iter()
        .filter(|i| ELIGIBLE.contains(&i.ownership_state))
        .cloned()
        .collect();

    let mut result = AdoptResult {
        from_repo_id: from_repo_id.to_owned(),
        items: Vec::new(),
    };
    let now = context::now_iso8601();

    for src_item in candidates {
        let src_file = src_store.join(&src_item.store_path);
        let dst_file = ctx.repo_store.join(&src_item.store_path);

        // Skip items already managed by the current repo at the same path.
        if ctx.manifest.contains(&src_item.path) {
            result.items.push(AdoptedItem {
                path: src_item.path,
                item_id: src_item.item_id,
                outcome: AdoptOutcome::Conflict,
            });
            continue;
        }

        // Skip items whose source store file is missing.
        if !src_file.exists() {
            result.items.push(AdoptedItem {
                path: src_item.path,
                item_id: src_item.item_id,
                outcome: AdoptOutcome::StoreMissing,
            });
            continue;
        }

        if dry_run {
            result.items.push(AdoptedItem {
                path: src_item.path,
                item_id: src_item.item_id,
                outcome: AdoptOutcome::WouldAdopt,
            });
            continue;
        }

        // Copy store file to the current repo's store.
        if let Some(parent) = dst_file.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }
        std::fs::copy(&src_file, &dst_file).map_err(|e| AppError::io(&dst_file, e))?;

        // Add to current manifest with preserved item_id and origin_repo_id.
        ctx.manifest.add(Item {
            item_id: src_item.item_id.clone(),
            origin_repo_id: src_item.origin_repo_id.clone(),
            path: src_item.path.clone(),
            store_path: src_item.store_path.clone(),
            kind: src_item.kind,
            link: src_item.link,
            git: src_item.git,
            ownership_state: OwnershipState::Attached,
            created_at: src_item.created_at.clone(),
            updated_at: now.clone(),
        });

        // Determine whether this is a reclaim (same logical repo) or a transfer.
        //
        // Current reclaim heuristic: same git_common_dir implies same logical repo.
        // Future ownership metadata (e.g. stable item lineage) may replace this.
        //
        // Spec §6.1:
        //   unreachable -> attached  (reclaim: same identity)
        //   unreachable -> adopted   (transfer: different identity)
        //   stale       -> adopted   (always transfer)
        let is_reclaim = src_git_common_dir == ctx.git_common_dir
            && src_item.ownership_state == OwnershipState::Unreachable;

        let (src_new_state, outcome_ok, outcome_link_fail) = if is_reclaim {
            (
                OwnershipState::Attached,
                AdoptOutcome::Reclaimed,
                AdoptOutcome::ReclaimedLinkFailed,
            )
        } else {
            (
                OwnershipState::Adopted,
                AdoptOutcome::Adopted,
                AdoptOutcome::AdoptedLinkFailed,
            )
        };

        // Mark source item with the appropriate post-adopt state.
        src_manifest.set_ownership_state(&src_item.path, src_new_state, &now);

        // Recreate symlink at the repo path pointing to the new store location.
        let abs_link = ctx.repo_root.join(&src_item.path);
        let outcome = if update_symlink(&abs_link, &dst_file, link).is_ok() {
            outcome_ok
        } else {
            outcome_link_fail
        };

        result.items.push(AdoptedItem {
            path: src_item.path,
            item_id: src_item.item_id,
            outcome,
        });
    }

    if !dry_run {
        manifest::save(&ctx.repo_store, &ctx.manifest)?;
        manifest::save(&src_store, &src_manifest)?;
    }

    Ok(result)
}

// ── Helpers ────────────────────────────────────────────────────────────────────

/// Creates or replaces the symlink at `abs_link` pointing to `target`.
///
/// - If a symlink already exists, it is removed first.
/// - If a regular file or directory exists, returns an error (no data loss).
fn update_symlink(abs_link: &Path, target: &Path, link: &dyn LinkStrategy) -> Result<()> {
    if let Ok(meta) = abs_link.symlink_metadata() {
        if meta.file_type().is_symlink() {
            link.remove(abs_link)?;
        } else {
            return Err(AppError::PathIsRegularFile {
                path: abs_link.to_path_buf(),
            });
        }
    }
    link.create(target, abs_link)
}
