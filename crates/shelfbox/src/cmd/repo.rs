use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::Serialize;
use shelfbox_core::{
    config::Config,
    context,
    ignore::GitInfoExclude,
    link::SymlinkStrategy,
    ops,
    ops::doctor::{DoctorReport, FixResult},
    store::{index, manifest},
};

use crate::cmd::format::OutputFormat;

// ── repo subcommands ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum RepoCommand {
    /// List all repositories known to the store.
    List {
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },

    /// Show the health status of the current repository's shelf.
    Status {
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },

    /// Apply safe automatic repairs (broken symlinks, exclude, root mismatch).
    Repair {
        /// Print what would happen without making any changes.
        #[arg(long)]
        dry_run: bool,
    },

    /// Delete orphan store items not referenced by the manifest.
    Gc {
        /// Print what would be deleted without making any changes.
        #[arg(long)]
        dry_run: bool,

        /// Skip confirmation prompt and perform deletions immediately.
        #[arg(long)]
        yes: bool,
    },

    /// Re-associate a repository after a reclone or path change (not yet implemented).
    Relink,

    /// Migrate the manifest schema to the current version (not yet implemented).
    Migrate,
}

// ── repo command runner ─────────────────────────────────────────────────────────────────────────

pub fn run_repo(command: RepoCommand, cwd: &Path, store_override: Option<&Path>) -> Result<()> {
    match command {
        RepoCommand::List { format } => cmd_repo_list(store_override, format),
        RepoCommand::Status { format } => cmd_repo_status(cwd, store_override, format),
        RepoCommand::Repair { dry_run } => cmd_repo_repair(cwd, store_override, dry_run),
        RepoCommand::Gc { dry_run, yes } => cmd_repo_gc(cwd, store_override, dry_run, yes),
        RepoCommand::Relink | RepoCommand::Migrate => {
            anyhow::bail!("not yet implemented")
        }
    }
}

// ── Subcommand handlers ─────────────────────────────────────────────────────────────────────────

/// Snapshot of a single repository entry for list output.
#[derive(Debug, Serialize)]
struct RepoSummary {
    name: String,
    root: PathBuf,
    item_count: usize,
    last_seen_at: String,
}

fn cmd_repo_list(store_override: Option<&Path>, format: OutputFormat) -> Result<()> {
    let config = Config::load(store_override).context("failed to load config")?;
    let idx = index::load(&config.store).context("failed to load store index")?;

    let mut rows: Vec<RepoSummary> = idx
        .iter()
        .map(|(_id, entry)| {
            let repo_store = config.store.join("repos").join(&entry.store_dir);
            let item_count = manifest::load(&repo_store)
                .map(|m| m.items.len())
                .unwrap_or(0);
            RepoSummary {
                name: entry
                    .root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| entry.root.to_string_lossy().into_owned()),
                root: entry.root.clone(),
                item_count,
                last_seen_at: entry.last_seen_at.clone(),
            }
        })
        .collect();

    // Stable sort by repository name for deterministic output.
    rows.sort_by(|a, b| a.name.cmp(&b.name));

    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&rows)?),
        OutputFormat::Plain => {
            for r in &rows {
                println!("{} {} {}", r.name, r.root.display(), r.item_count);
            }
        }
        OutputFormat::Table => {
            if rows.is_empty() {
                println!("(no repositories in store)");
                return Ok(());
            }
            println!("  {:<30} {:<50} {:>5}  last seen", "name", "root", "items");
            println!("  {}", "-".repeat(100));
            for r in &rows {
                println!(
                    "  {:<30} {:<50} {:>5}  {}",
                    r.name,
                    r.root.display(),
                    r.item_count,
                    r.last_seen_at,
                );
            }
        }
        OutputFormat::Detail => anyhow::bail!("--format detail is not yet implemented"),
    }
    Ok(())
}

fn cmd_repo_status(cwd: &Path, store_override: Option<&Path>, format: OutputFormat) -> Result<()> {
    let ctx =
        context::build(cwd, store_override, false).context("failed to initialise repo context")?;
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;
    let report = ops::doctor::doctor(&ctx, &link, &ignore)?;

    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&report)?),
        OutputFormat::Plain => print_repo_status_plain(&report),
        OutputFormat::Table => print_repo_status(&report, &ctx.repo_root),
        OutputFormat::Detail => anyhow::bail!("--format detail is not yet implemented"),
    }
    Ok(())
}

fn cmd_repo_repair(cwd: &Path, store_override: Option<&Path>, dry_run: bool) -> Result<()> {
    let mut ctx =
        context::build(cwd, store_override, true).context("failed to initialise repo context")?;
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    // yes=false: safe fixes only; orphan deletion requires explicit `repo gc`.
    let report = ops::doctor::doctor_fix(&mut ctx, &link, &ignore, false, dry_run)?;

    for action in &report.actions {
        print_fix_result(action);
    }
    for warning in &report.data_loss_warnings {
        eprintln!("warning: data loss — {warning}");
    }
    Ok(())
}

fn cmd_repo_gc(cwd: &Path, store_override: Option<&Path>, dry_run: bool, yes: bool) -> Result<()> {
    let ctx =
        context::build(cwd, store_override, true).context("failed to initialise repo context")?;
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    // Use doctor() to collect orphan store items.
    let report = ops::doctor::doctor(&ctx, &link, &ignore)?;

    if report.orphan_store_items.is_empty() {
        println!("no orphan store items found");
        return Ok(());
    }

    if dry_run {
        println!("orphan store items that would be deleted:");
        for orphan in &report.orphan_store_items {
            println!("  {orphan}");
        }
        return Ok(());
    }

    if !yes {
        println!("orphan store items:");
        for orphan in &report.orphan_store_items {
            println!("  {orphan}");
        }
        println!("re-run with --yes to delete them");
        return Ok(());
    }

    // Delete confirmed.
    for orphan in &report.orphan_store_items {
        let full_path = ctx.items_dir().join(orphan);
        let result = if full_path.is_dir() {
            std::fs::remove_dir_all(&full_path)
        } else {
            std::fs::remove_file(&full_path)
        };
        match result {
            Ok(()) => println!("deleted: {orphan}"),
            Err(e) => eprintln!("error: failed to delete '{orphan}': {e}"),
        }
    }
    Ok(())
}

// ── Human-readable formatters ───────────────────────────────────────────────────────────────────

fn print_repo_status(report: &DoctorReport, repo_root: &Path) {
    println!("repo: {}", repo_root.display());

    let total = report.items.len();
    let errors = report.items.iter().filter(|s| !s.ok).count();
    let overall = if errors > 0 { "ERROR" } else { "OK" };

    println!("items: {total} total, {errors} with issues  [{overall}]");

    for s in &report.items {
        let mut issues: Vec<&str> = Vec::new();
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
            issues.push("tracked by git");
        }
        let label = if s.ok { "OK" } else { "ERROR" };
        if issues.is_empty() {
            println!("  {label:<8} {}", s.path);
        } else {
            println!("  {label:<8} {}  ({})", s.path, issues.join(", "));
        }
    }

    let orphan_count = report.orphan_store_items.len();
    if orphan_count > 0 {
        println!("orphan store items: {orphan_count}  [WARN]");
        for o in &report.orphan_store_items {
            println!("  {o}");
        }
    } else {
        println!("orphan store items: 0  [OK]");
    }

    let root_label = if report.repo_root_matches_index {
        "OK"
    } else {
        "WARN"
    };
    println!("index root: [{root_label}]");
}

fn print_repo_status_plain(report: &DoctorReport) {
    for s in &report.items {
        let label = if s.ok { "OK" } else { "ERROR" };
        println!("{label} {}", s.path);
    }
    for o in &report.orphan_store_items {
        println!("ORPHAN {o}");
    }
    if !report.repo_root_matches_index {
        println!("ROOT_MISMATCH");
    }
}

fn print_fix_result(result: &FixResult) {
    match result {
        FixResult::Fixed(msg) => println!("  fixed    {msg}"),
        FixResult::Skipped(msg) => println!("  skipped  {msg}"),
        FixResult::Failed(msg) => eprintln!("  failed   {msg}"),
        FixResult::NeedsConfirmation(msg) => println!("  confirm  {msg}"),
        FixResult::CannotFix(msg) => eprintln!("  cannot   {msg}"),
    }
}
