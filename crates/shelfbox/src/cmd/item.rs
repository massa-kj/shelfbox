use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Subcommand;
use shelfbox_core::{
    context,
    error::AppError,
    ignore::GitInfoExclude,
    link::SymlinkStrategy,
    ops,
    ops::{
        add::{DirItemOutcome, SkipReason},
        info::ItemInfo,
        restore::NsRestoreItemOutcome,
        status::ItemStatus,
    },
    store::manifest::{Item, ItemKind},
};

use crate::cmd::format::OutputFormat;
use crate::cmd::util::resolve_path;

// ── item subcommands ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum ItemCommand {
    /// Move a file into the store and leave a symlink in its place.
    Add {
        /// Files to shelve (relative to repo root).
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,

        /// Print what would happen without making any changes.
        #[arg(long)]
        dry_run: bool,
    },

    /// Return a shelved file to its original location and remove it from the store.
    Restore {
        /// Files to restore (relative to repo root).
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,

        /// Print what would happen without making any changes.
        #[arg(long)]
        dry_run: bool,

        /// Keep the .git/info/exclude entry after restoring.
        #[arg(long)]
        keep_ignore: bool,

        /// Remove from manifest only; keep the store item and symlink in place.
        /// The store item becomes an orphan subject to `repo gc`.
        #[arg(long)]
        keep_store: bool,
    },

    /// Recreate a missing or broken symlink for one or more shelved files.
    Repair {
        /// Files to repair (relative to repo root).
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,

        /// Print what would happen without making any changes.
        #[arg(long)]
        dry_run: bool,

        /// Allow overwriting a symlink that points to an unexpected target.
        /// Without this flag, `repair` refuses to touch wrong-target symlinks
        /// to avoid silently masking stale links from reclones or copied repos.
        #[arg(long)]
        force: bool,
    },

    /// Re-attach a detached item by recreating its symlink.
    ///
    /// A detached item is one whose ownership was intentionally unlinked via
    /// `item restore --keep-store`.  `relink` transitions the item from
    /// `detached` back to `attached` and recreates the symlink if needed.
    Relink {
        /// Files to relink (relative to repo root).
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,

        /// Print what would happen without making any changes.
        #[arg(long)]
        dry_run: bool,
    },

    /// List all shelved files for the current repository.
    List {
        /// Output format.
        #[arg(long, value_enum)]
        format: Option<OutputFormat>,
        /// Show extended fields (store path, symlink target).
        #[arg(long)]
        verbose: bool,
    },

    /// Show the health status of each shelved file.
    Status {
        /// Output format.
        #[arg(long, value_enum)]
        format: Option<OutputFormat>,
        /// Show extended fields for each item.
        #[arg(long)]
        verbose: bool,
    },

    /// Rename a shelved item's tracked path.
    Move {
        #[arg(value_name = "OLD")]
        old: PathBuf,

        #[arg(value_name = "NEW")]
        new_path: PathBuf,

        /// Print what would happen without making any changes.
        #[arg(long)]
        dry_run: bool,
    },

    /// Show metadata for a shelved item.
    Info {
        /// File to inspect (relative to repo root).
        #[arg(value_name = "PATH")]
        path: PathBuf,

        /// Output format.
        #[arg(long, value_enum)]
        format: Option<OutputFormat>,
    },
}

// ── item command runner ─────────────────────────────────────────────────────────────────────────

pub fn run_item(
    command: ItemCommand,
    cwd: &Path,
    store_override: Option<&Path>,
) -> Result<ExitCode> {
    match command {
        ItemCommand::Add { paths, dry_run } => {
            cmd_add(cwd, store_override, &paths, dry_run)?;
            Ok(ExitCode::SUCCESS)
        }
        ItemCommand::Restore {
            paths,
            dry_run,
            keep_ignore,
            keep_store,
        } => {
            cmd_restore(
                cwd,
                store_override,
                &paths,
                dry_run,
                keep_ignore,
                keep_store,
            )?;
            Ok(ExitCode::SUCCESS)
        }
        ItemCommand::Repair {
            paths,
            dry_run,
            force,
        } => {
            cmd_repair(cwd, store_override, &paths, dry_run, force)?;
            Ok(ExitCode::SUCCESS)
        }
        ItemCommand::Relink { paths, dry_run } => {
            cmd_relink(cwd, store_override, &paths, dry_run)?;
            Ok(ExitCode::SUCCESS)
        }
        ItemCommand::List { format, verbose } => {
            cmd_list(cwd, store_override, format, verbose)?;
            Ok(ExitCode::SUCCESS)
        }
        ItemCommand::Status { format, verbose } => cmd_status(cwd, store_override, format, verbose),
        ItemCommand::Move {
            old,
            new_path,
            dry_run,
        } => {
            cmd_move(cwd, store_override, &old, &new_path, dry_run)?;
            Ok(ExitCode::SUCCESS)
        }
        ItemCommand::Info { path, format } => {
            cmd_info(cwd, store_override, &path, format)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

// ── Subcommand handlers ─────────────────────────────────────────────────────────────────────────

fn cmd_add(
    cwd: &Path,
    store_override: Option<&Path>,
    paths: &[PathBuf],
    dry_run: bool,
) -> Result<()> {
    let mut ctx =
        context::build(cwd, store_override, true).context("failed to initialise repo context")?;
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    for path in paths {
        let abs = resolve_path(cwd, path);

        if abs.is_dir() {
            // Directory namespace add: shelve all eligible files inside.
            let result = ops::add::add_directory(&mut ctx, &abs, dry_run, &link, &ignore)
                .with_context(|| format!("add '{}' failed", path.display()))?;
            print_dir_add_result(&result);
        } else {
            // Single-file add.
            match ops::add::add(&mut ctx, &abs, dry_run, &link, &ignore) {
                Ok(()) => {}
                // Special-case: give the user an actionable hint for tracked files.
                Err(AppError::PathIsTracked { path: ref p }) => {
                    let rel = p
                        .strip_prefix(cwd)
                        .unwrap_or(p.as_path())
                        .display()
                        .to_string();
                    eprintln!("error: '{rel}' is tracked by git");
                    eprintln!("hint: remove it from the index first:");
                    eprintln!("  git rm --cached {rel}");
                    eprintln!("then re-run: shelfbox add {rel}");
                    return Err(anyhow::anyhow!("add '{rel}' failed"));
                }
                Err(e) => {
                    return Err(e).with_context(|| format!("add '{}' failed", path.display()));
                }
            }
            if !dry_run {
                println!("shelved: {}", path.display());
            }
        }
    }
    Ok(())
}

/// Prints a human-readable summary of a directory add operation.
fn print_dir_add_result(result: &ops::add::DirectoryAddResult) {
    let added: Vec<&str> = result
        .results
        .iter()
        .filter(|(_, o)| matches!(o, DirItemOutcome::Added | DirItemOutcome::WouldAdd))
        .map(|(p, _)| p.as_str())
        .collect();
    let skipped: Vec<(&str, &SkipReason)> = result
        .results
        .iter()
        .filter_map(|(p, o)| {
            if let DirItemOutcome::Skipped(reason) = o {
                Some((p.as_str(), reason))
            } else {
                None
            }
        })
        .collect();
    let nested: Vec<&str> = result
        .results
        .iter()
        .filter(|(_, o)| matches!(o, DirItemOutcome::NestedGitRepo))
        .map(|(p, _)| p.as_str())
        .collect();
    let failed: Vec<(&str, &str)> = result
        .results
        .iter()
        .filter_map(|(p, o)| {
            if let DirItemOutcome::Failed(msg) = o {
                Some((p.as_str(), msg.as_str()))
            } else {
                None
            }
        })
        .collect();

    let is_dry_run = result
        .results
        .iter()
        .any(|(_, o)| matches!(o, DirItemOutcome::WouldAdd));
    let prefix = if is_dry_run { "[dry-run] " } else { "" };

    println!(
        "{}namespace '{}': {} added, {} skipped, {} failed",
        prefix,
        result.ns_path,
        added.len(),
        skipped.len(),
        failed.len() + nested.len()
    );
    for path in &added {
        println!("  {}shelved: {path}", prefix);
    }
    for (path, reason) in &skipped {
        println!("  skip: {path} ({reason})");
    }
    for path in &nested {
        eprintln!("  skip: {path} (nested git repository — not crossed)");
    }
    for (path, msg) in &failed {
        eprintln!("  fail: {path}: {msg}");
    }
    if result.namespace_created {
        println!("namespace registered: {}", result.ns_path);
    }
}

fn cmd_restore(
    cwd: &Path,
    store_override: Option<&Path>,
    paths: &[PathBuf],
    dry_run: bool,
    keep_ignore: bool,
    keep_store: bool,
) -> Result<()> {
    let mut ctx =
        context::build(cwd, store_override, true).context("failed to initialise repo context")?;
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    for path in paths {
        let abs = resolve_path(cwd, path);

        // Detect namespace restore: path ends with "/" or abs path is a directory.
        let path_str = path.to_string_lossy();
        let is_namespace = path_str.ends_with('/') || abs.is_dir();

        if is_namespace {
            let rel = abs
                .strip_prefix(&ctx.repo_root)
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path_str.trim_end_matches('/').to_owned());
            let ns_path = if rel.ends_with('/') {
                rel
            } else {
                format!("{rel}/")
            };

            let result = ops::restore::restore_namespace(
                &mut ctx,
                &ns_path,
                dry_run,
                keep_ignore,
                keep_store,
                &link,
                &ignore,
            )
            .with_context(|| format!("restore namespace '{}' failed", ns_path))?;
            print_ns_restore_result(&result);
        } else {
            ops::restore::restore(
                &mut ctx,
                &abs,
                dry_run,
                keep_ignore,
                keep_store,
                &link,
                &ignore,
            )
            .with_context(|| format!("restore '{}' failed", path.display()))?;
            if !dry_run {
                println!("restored: {}", path.display());
            }
        }
    }
    Ok(())
}

/// Prints a human-readable summary of a namespace restore operation.
fn print_ns_restore_result(result: &ops::restore::NamespaceRestoreResult) {
    let is_dry_run = result
        .results
        .iter()
        .any(|(_, o)| matches!(o, NsRestoreItemOutcome::WouldRestore));
    let prefix = if is_dry_run { "[dry-run] " } else { "" };

    let restored = result
        .results
        .iter()
        .filter(|(_, o)| {
            matches!(
                o,
                NsRestoreItemOutcome::Restored | NsRestoreItemOutcome::WouldRestore
            )
        })
        .count();
    let failed = result
        .results
        .iter()
        .filter(|(_, o)| matches!(o, NsRestoreItemOutcome::Failed(_)))
        .count();

    println!(
        "{}namespace '{}': {} restored, {} failed",
        prefix, result.ns_path, restored, failed
    );
    for (path, outcome) in &result.results {
        match outcome {
            NsRestoreItemOutcome::Restored => println!("  {}restored: {path}", prefix),
            NsRestoreItemOutcome::WouldRestore => println!("  {}restore: {path}", prefix),
            NsRestoreItemOutcome::Failed(msg) => eprintln!("  fail: {path}: {msg}"),
        }
    }
    if result.namespace_removed {
        println!("namespace removed: {}", result.ns_path);
    }
}

fn cmd_list(
    cwd: &Path,
    store_override: Option<&Path>,
    format: Option<OutputFormat>,
    verbose: bool,
) -> Result<()> {
    let ctx =
        context::build(cwd, store_override, false).context("failed to initialise repo context")?;
    let fmt = OutputFormat::resolve(format, &ctx.config.default_format);
    let items = ops::list::list(&ctx);

    match fmt {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(items)?),
        OutputFormat::Plain => print_list_plain(items),
        OutputFormat::Table => print_list(items, verbose, &ctx),
    }
    Ok(())
}

fn cmd_status(
    cwd: &Path,
    store_override: Option<&Path>,
    format: Option<OutputFormat>,
    verbose: bool,
) -> Result<ExitCode> {
    let ctx =
        context::build(cwd, store_override, false).context("failed to initialise repo context")?;
    let fmt = OutputFormat::resolve(format, &ctx.config.default_format);
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;
    let statuses = ops::status::status(&ctx, &link, &ignore)?;

    match fmt {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&statuses)?),
        OutputFormat::Plain => print_status_plain(&statuses),
        OutputFormat::Table => print_status(&statuses, verbose, &ctx),
    }
    Ok(classify_status_exit(&statuses))
}

fn cmd_repair(
    cwd: &Path,
    store_override: Option<&Path>,
    paths: &[PathBuf],
    dry_run: bool,
    force: bool,
) -> Result<()> {
    let ctx =
        context::build(cwd, store_override, true).context("failed to initialise repo context")?;
    let link = SymlinkStrategy;

    for path in paths {
        let abs = resolve_path(cwd, path);
        match ops::repair::repair(&ctx, &abs, &link, dry_run, force)
            .with_context(|| format!("repair '{}' failed", path.display()))?
        {
            ops::repair::RepairOutcome::LinkRecreated => {
                if !dry_run {
                    println!("repaired: {}", path.display());
                }
            }
            ops::repair::RepairOutcome::AlreadyHealthy => {
                println!("ok (no repair needed): {}", path.display());
            }
            ops::repair::RepairOutcome::StoreMissing => {
                eprintln!(
                    "error: store item missing for '{}' — data may be lost. \
                     Restore manually and re-add.",
                    path.display()
                );
            }
            ops::repair::RepairOutcome::NotManaged => {
                eprintln!("error: '{}' is not managed by shelfbox", path.display());
            }
        }
    }
    Ok(())
}

fn cmd_relink(
    cwd: &Path,
    store_override: Option<&Path>,
    paths: &[PathBuf],
    dry_run: bool,
) -> Result<()> {
    let mut ctx =
        context::build(cwd, store_override, true).context("failed to initialise repo context")?;
    let link = SymlinkStrategy;

    for path in paths {
        let abs = resolve_path(cwd, path);
        match ops::relink::relink(&mut ctx, &abs, dry_run, &link)
            .with_context(|| format!("relink '{}' failed", path.display()))?
        {
            ops::relink::RelinkOutcome::Relinked => {
                println!("relinked: {}", path.display());
            }
            ops::relink::RelinkOutcome::StateUpdated => {
                println!("relinked (symlink already correct): {}", path.display());
            }
            ops::relink::RelinkOutcome::WouldRelink => {}
        }
    }
    Ok(())
}

fn cmd_move(
    cwd: &Path,
    store_override: Option<&Path>,
    old: &Path,
    new_path: &Path,
    dry_run: bool,
) -> Result<()> {
    let old_abs = resolve_path(cwd, old);
    let new_abs = resolve_path(cwd, new_path);
    let mut ctx =
        context::build(cwd, store_override, true).context("failed to initialise repo context")?;
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;
    ops::move_item::move_item(&mut ctx, &old_abs, &new_abs, dry_run, &link, &ignore)
        .with_context(|| format!("move '{}' → '{}' failed", old.display(), new_path.display()))?;
    if !dry_run {
        println!("moved: {} → {}", old.display(), new_path.display());
    }
    Ok(())
}

// ── Human-readable formatters ───────────────────────────────────────────────────────────────────

/// Plain format: one path per line.
fn print_list_plain(items: &[Item]) {
    for item in items {
        println!("{}", item.path);
    }
}

fn print_list(items: &[Item], verbose: bool, ctx: &context::RepoContext) {
    if items.is_empty() {
        println!("(no shelved items)");
        return;
    }
    for item in items {
        let kind = match item.kind {
            ItemKind::File => "file",
            ItemKind::Directory => "dir",
        };
        println!("  {:<45} {:<5} {}", item.path, kind, item.created_at);
        if verbose {
            let store_path = ctx.repo_store.join(&item.store_path);
            let link_target = std::fs::read_link(ctx.repo_root.join(&item.path)).ok();
            println!("    store:  {}", store_path.display());
            match link_target {
                Some(t) => println!("    link\u{2192}   {}", t.display()),
                None => println!("    link\u{2192}   (none)"),
            }
        }
    }
}

/// Plain format: `label path [issue,issue,...]`
fn print_status_plain(statuses: &[ItemStatus]) {
    for s in statuses {
        let (label, issues) = classify_status(s);
        if issues.is_empty() {
            println!("{} {}", label, s.path);
        } else {
            println!("{} {} {}", label, s.path, issues.join(","));
        }
    }
}

fn print_status(statuses: &[ItemStatus], verbose: bool, ctx: &context::RepoContext) {
    if statuses.is_empty() {
        println!("(no shelved items)");
        return;
    }
    for (s, item) in statuses.iter().zip(ctx.manifest.items.iter()) {
        let (label, issues) = classify_status(s);
        if issues.is_empty() {
            println!("{:<8} {}", label, s.path);
        } else {
            println!("{:<8} {}  ({})", label, s.path, issues.join(", "));
        }
        if verbose {
            let store_path = ctx.repo_store.join(&item.store_path);
            let link_target = std::fs::read_link(ctx.repo_root.join(&s.path)).ok();
            println!("    store:        {}", store_path.display());
            match link_target {
                Some(t) => println!("    link\u{2192}         {}", t.display()),
                None => println!("    link\u{2192}         (none)"),
            }
            println!("    link_valid:   {}", s.link_valid);
            println!("    store_exists: {}", s.store_exists);
            println!("    in_exclude:   {}", s.in_exclude);
            println!("    not_tracked:  {}", s.not_tracked);
        }
    }
}

/// Returns `(severity_label, list_of_problem_descriptions)` for one item.
///
/// Severity rules:
/// - ERROR: any structural failure (symlink missing/invalid, store item gone,
///   or Git can see the file — the primary shelfbox contract is broken).
/// - WARN:  exclude entry missing but Git still ignores the file for now.
/// - OK:    all checks pass.
fn classify_status(s: &ItemStatus) -> (&'static str, Vec<&'static str>) {
    let mut issues: Vec<&'static str> = Vec::new();

    if !s.link_exists {
        issues.push("symlink missing");
    } else if !s.link_valid {
        issues.push("symlink invalid");
    }
    if !s.store_exists {
        issues.push("store item missing");
    }
    if !s.in_exclude {
        issues.push("not in exclude");
    }
    if !s.not_tracked {
        // Git can see the shelved file — the primary shelfbox contract
        // ("hide from Git") is broken.  This warrants ERROR, not WARN.
        issues.push("tracked by git");
    }

    let label = if !s.link_exists || !s.link_valid || !s.store_exists || !s.not_tracked {
        "ERROR"
    } else if !issues.is_empty() {
        // Only !in_exclude remains: the symlink is healthy and Git does not
        // currently track the file, but the exclude entry is gone.  A future
        // `git add .` could stage it, so this is a real warning.
        "WARN"
    } else {
        "OK"
    };

    (label, issues)
}

/// Determine the exit code for `item status` based on the item statuses.
///
/// - 2: structural ERROR (broken/missing symlink, missing store item, git-tracked)
/// - 1: WARN only (exclude entry missing)
/// - 0: all clear
fn classify_status_exit(statuses: &[ItemStatus]) -> ExitCode {
    let has_error = statuses
        .iter()
        .any(|s| !s.link_exists || !s.link_valid || !s.store_exists || !s.not_tracked);
    if has_error {
        return ExitCode::from(2);
    }

    let has_warn = statuses.iter().any(|s| !s.in_exclude);
    if has_warn {
        return ExitCode::from(1);
    }

    ExitCode::SUCCESS
}

// ── item info ───────────────────────────────────────────────────────────────

fn cmd_info(
    cwd: &Path,
    store_override: Option<&Path>,
    path: &Path,
    format: Option<OutputFormat>,
) -> Result<()> {
    let ctx =
        context::build(cwd, store_override, false).context("failed to initialise repo context")?;
    let fmt = OutputFormat::resolve(format, &ctx.config.default_format);
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;
    let abs = resolve_path(cwd, path);
    let item_info = ops::info::info(&ctx, &abs, &link, &ignore)?;

    match fmt {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&item_info)?),
        OutputFormat::Plain => {
            if let Some(ref sp) = item_info.store_path {
                println!("{}", sp.display());
            }
        }
        OutputFormat::Table => print_info_table(&item_info),
    }
    Ok(())
}

fn print_info_table(info: &ItemInfo) {
    println!("{:<14} {}", "path:", info.path);
    println!("{:<14} {}", "repo_root:", info.repo_root.display());
    println!(
        "{:<14} {}",
        "store_path:",
        info.store_path
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(not in manifest)".to_string())
    );
    println!(
        "{:<14} {}",
        "link_target:",
        info.link_target
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(no symlink)".to_string())
    );
    println!("{:<14} {}", "symlink_ok:", info.symlink_ok);
    println!("{:<14} {}", "tracked:", info.tracked);
    println!("{:<14} {}", "in_exclude:", info.in_exclude);
}
