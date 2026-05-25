use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Subcommand;
use shelfbox_core::{
    context,
    error::AppError,
    ignore::GitInfoExclude,
    link::SymlinkStrategy,
    ops,
    ops::{info::ItemInfo, status::ItemStatus},
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
    },

    /// List all shelved files for the current repository.
    List {
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },

    /// Show the health status of each shelved file.
    Status {
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
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
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
}

// ── item command runner ─────────────────────────────────────────────────────────────────────────

pub fn run_item(command: ItemCommand, cwd: &Path, store_override: Option<&Path>) -> Result<()> {
    match command {
        ItemCommand::Add { paths, dry_run } => cmd_add(cwd, store_override, &paths, dry_run),
        ItemCommand::Restore {
            paths,
            dry_run,
            keep_ignore,
            keep_store,
        } => cmd_restore(
            cwd,
            store_override,
            &paths,
            dry_run,
            keep_ignore,
            keep_store,
        ),
        ItemCommand::Repair { paths, dry_run } => cmd_repair(cwd, store_override, &paths, dry_run),
        ItemCommand::List { format } => cmd_list(cwd, store_override, format),
        ItemCommand::Status { format } => cmd_status(cwd, store_override, format),
        ItemCommand::Move {
            old,
            new_path,
            dry_run,
        } => cmd_move(cwd, store_override, &old, &new_path, dry_run),
        ItemCommand::Info { path, format } => cmd_info(cwd, store_override, &path, format),
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
    Ok(())
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
    Ok(())
}

fn cmd_list(cwd: &Path, store_override: Option<&Path>, format: OutputFormat) -> Result<()> {
    let ctx =
        context::build(cwd, store_override, false).context("failed to initialise repo context")?;
    let items = ops::list::list(&ctx);

    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(items)?),
        OutputFormat::Plain => print_list_plain(items),
        OutputFormat::Table => print_list(items),
        OutputFormat::Detail => print_list_detail(items, &ctx),
    }
    Ok(())
}

fn cmd_status(cwd: &Path, store_override: Option<&Path>, format: OutputFormat) -> Result<()> {
    let ctx =
        context::build(cwd, store_override, false).context("failed to initialise repo context")?;
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;
    let statuses = ops::status::status(&ctx, &link, &ignore)?;

    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&statuses)?),
        OutputFormat::Plain => print_status_plain(&statuses),
        OutputFormat::Table => print_status(&statuses),
        OutputFormat::Detail => print_status_detail(&statuses, &ctx),
    }
    Ok(())
}

fn cmd_repair(
    cwd: &Path,
    store_override: Option<&Path>,
    paths: &[PathBuf],
    dry_run: bool,
) -> Result<()> {
    let ctx =
        context::build(cwd, store_override, true).context("failed to initialise repo context")?;
    let link = SymlinkStrategy;

    for path in paths {
        let abs = resolve_path(cwd, path);
        match ops::repair::repair(&ctx, &abs, &link, dry_run)
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

fn print_list(items: &[Item]) {
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

fn print_status(statuses: &[ItemStatus]) {
    if statuses.is_empty() {
        println!("(no shelved items)");
        return;
    }
    for s in statuses {
        let (label, issues) = classify_status(s);
        if issues.is_empty() {
            println!("{:<8} {}", label, s.path);
        } else {
            println!("{:<8} {}  ({})", label, s.path, issues.join(", "));
        }
    }
}

/// Detail format: one block per item, showing all fields.
fn print_list_detail(items: &[Item], ctx: &context::RepoContext) {
    if items.is_empty() {
        println!("(no shelved items)");
        return;
    }
    for item in items {
        let kind = match item.kind {
            ItemKind::File => "file",
            ItemKind::Directory => "dir",
        };
        let store_path = ctx.repo_store.join(&item.store_path);
        let link_target = std::fs::read_link(ctx.repo_root.join(&item.path)).ok();
        println!("  {:<45} {:<5} {}", item.path, kind, item.created_at);
        println!("    store:  {}", store_path.display());
        match link_target {
            Some(t) => println!("    link→   {}", t.display()),
            None => println!("    link→   (none)"),
        }
    }
}

/// Detail format: one block per item, showing all health fields.
fn print_status_detail(statuses: &[ItemStatus], ctx: &context::RepoContext) {
    if statuses.is_empty() {
        println!("(no shelved items)");
        return;
    }
    for (s, item) in statuses.iter().zip(ctx.manifest.items.iter()) {
        let label = if s.ok { "OK" } else { "ERROR" };
        println!("  {label:<8} {}", s.path);
        let store_path = ctx.repo_store.join(&item.store_path);
        println!("    store:        {}", store_path.display());
        let link_target = std::fs::read_link(ctx.repo_root.join(&s.path)).ok();
        match link_target {
            Some(t) => println!("    link→         {}", t.display()),
            None => println!("    link→         (none)"),
        }
        println!("    link_valid:   {}", s.link_valid);
        println!("    store_exists: {}", s.store_exists);
        println!("    in_exclude:   {}", s.in_exclude);
        println!("    not_tracked:  {}", s.not_tracked);
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

// ── item info ───────────────────────────────────────────────────────────────

fn cmd_info(
    cwd: &Path,
    store_override: Option<&Path>,
    path: &Path,
    format: OutputFormat,
) -> Result<()> {
    let ctx =
        context::build(cwd, store_override, false).context("failed to initialise repo context")?;
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;
    let abs = resolve_path(cwd, path);
    let item_info = ops::info::info(&ctx, &abs, &link, &ignore)?;

    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&item_info)?),
        OutputFormat::Plain => {
            if let Some(ref sp) = item_info.store_path {
                println!("{}", sp.display());
            }
        }
        OutputFormat::Table | OutputFormat::Detail => print_info_table(&item_info),
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
