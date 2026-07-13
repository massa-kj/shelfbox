use std::{fs, path::Path};

pub use crate::{
    context::{ReadOnlyRepoContext, RepoContext},
    error::AppError,
    ops::{
        add::{DirItemOutcome, DirectoryAddResult, SkipReason},
        info::ItemInfo,
        relink::RelinkOutcome,
        restore::{NamespaceRestoreResult, NsRestoreItemOutcome},
        status::{
            CopyContentState, ItemStatus, ItemStatusV2, MaterializationStrategy,
            ObservedMaterialization, StatusIssue, StatusIssueCode, StatusNote, StatusNoteCode,
            StatusOptions, StatusSchemaVersion, StatusSeverity, STATUS_SCHEMA_VERSION_V2,
        },
    },
    plan::{
        item_add::{ItemAddPlan, ItemAddReport},
        item_move::{ItemMovePlan, ItemMoveReport, ItemMoveWarning},
        item_relink::{ItemRelinkPlan, ItemRelinkReport},
        item_repair::{ItemRepairReport, RepairOutcome},
        item_restore::{ItemRestoreAction, ItemRestorePlan, ItemRestoreReport},
        repo_repair::{RepoRepairAction, RepoRepairSymlinkAction},
    },
    store::manifest::Item,
};

use crate::{
    context,
    error::Result,
    ignore::{GitInfoExclude, IgnoreBackend},
    link::DefaultLinkStrategy,
    ops::{
        add, info as info_ops, list as list_ops, move_item as move_item_ops, path as path_ops,
        relink as relink_ops, repair as repair_ops, restore, status as status_ops,
    },
};

pub fn build_create_or_load(cwd: &Path, store_override: Option<&Path>) -> Result<RepoContext> {
    context::build_create_or_load(cwd, store_override)
}

pub fn build_preview_create_or_load(
    cwd: &Path,
    store_override: Option<&Path>,
) -> Result<RepoContext> {
    context::build_preview_create_or_load(cwd, store_override)
}

pub fn build_read_only(cwd: &Path, store_override: Option<&Path>) -> Result<ReadOnlyRepoContext> {
    context::build_read_only(cwd, store_override)
}

pub fn add_file(ctx: &mut RepoContext, abs_path: &Path, dry_run: bool) -> Result<ItemAddReport> {
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    add::add_report(ctx, abs_path, dry_run, &link, &ignore)
}

pub fn add_directory(
    ctx: &mut RepoContext,
    abs_path: &Path,
    dry_run: bool,
) -> Result<DirectoryAddResult> {
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    add::add_directory(ctx, abs_path, dry_run, &link, &ignore)
}

pub fn restore_file(
    ctx: &mut RepoContext,
    abs_path: &Path,
    dry_run: bool,
    keep_ignore: bool,
    keep_store: bool,
) -> Result<ItemRestoreReport> {
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    restore::restore(
        ctx,
        abs_path,
        dry_run,
        keep_ignore,
        keep_store,
        &link,
        &ignore,
    )
}

pub fn restore_namespace(
    ctx: &mut RepoContext,
    ns_path: &str,
    dry_run: bool,
    keep_ignore: bool,
    keep_store: bool,
) -> Result<NamespaceRestoreResult> {
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    restore::restore_namespace(
        ctx,
        ns_path,
        dry_run,
        keep_ignore,
        keep_store,
        &link,
        &ignore,
    )
}

pub fn list(ctx: &RepoContext) -> &[Item] {
    list_ops::list(ctx)
}

pub fn status(ctx: &RepoContext) -> Result<Vec<ItemStatus>> {
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    status_ops::status(ctx, &link, &ignore)
}

pub fn status_v2(ctx: &RepoContext, options: StatusOptions) -> Result<Vec<ItemStatusV2>> {
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    status_ops::status_v2(ctx, &link, &ignore, options)
}

pub fn repair(
    ctx: &RepoContext,
    abs_path: &Path,
    dry_run: bool,
    force: bool,
) -> Result<ItemRepairReport> {
    let link = DefaultLinkStrategy;
    repair_ops::repair_report(ctx, abs_path, &link, dry_run, force)
}

pub fn relink(ctx: &mut RepoContext, abs_path: &Path, dry_run: bool) -> Result<ItemRelinkReport> {
    let link = DefaultLinkStrategy;
    relink_ops::relink_report(ctx, abs_path, dry_run, &link)
}

pub fn move_item(
    ctx: &mut RepoContext,
    old_abs: &Path,
    new_abs: &Path,
    dry_run: bool,
) -> Result<ItemMoveReport> {
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    move_item_ops::move_item(ctx, old_abs, new_abs, dry_run, &link, &ignore)
}

pub fn info(ctx: &RepoContext, abs_path: &Path) -> Result<ItemInfo> {
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    info_ops::info(ctx, abs_path, &link, &ignore)
}

pub fn info_read_only(read_only: &ReadOnlyRepoContext, abs_path: &Path) -> Result<ItemInfo> {
    if let Some(ctx) = &read_only.repo {
        return info(ctx, abs_path);
    }

    let rel_str = path_ops::repo_relative_string(&read_only.current.repo_root, abs_path)?;
    let ignore = GitInfoExclude;
    Ok(ItemInfo {
        path: rel_str.clone(),
        repo_root: read_only.current.repo_root.clone(),
        store_path: None,
        link_target: fs::read_link(abs_path).ok(),
        symlink_ok: false,
        tracked: false,
        in_exclude: ignore.has_entry(&read_only.current.repo_root, &rel_str)?,
    })
}
