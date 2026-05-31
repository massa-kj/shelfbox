use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::Serialize;
use shelfbox_core::{
    config::Config,
    context,
    ignore::GitInfoExclude,
    link::SymlinkStrategy,
    ops,
    ops::{
        adopt::AdoptOutcome,
        integrity::{FixResult, IntegrityReport},
    },
    store::{index, manifest},
};

use crate::cmd::format::OutputFormat;

// ── repo subcommands ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum RepoCommand {
    /// List all repositories known to the store.
    List {
        /// Output format.
        #[arg(long, value_enum)]
        format: Option<OutputFormat>,
        /// Show extended fields for each repository.
        #[arg(long)]
        verbose: bool,
    },

    /// Show the health status of the current repository's shelf.
    ///
    /// Exit codes: 0 = all OK, 1 = warnings only, 2 = errors present.
    Status {
        /// Output format.
        #[arg(long, value_enum)]
        format: Option<OutputFormat>,
        /// Show extended fields for each item.
        #[arg(long)]
        verbose: bool,
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

    /// Adopt shelved items from another repository into the current one.
    ///
    /// Use this after a reclone or repository rename to reclaim shelved items
    /// from the old identity.  Specify the source repository by its ID as
    /// shown by `shelfbox repo list --verbose`.
    Adopt {
        /// Source repository ID to adopt items from.
        #[arg(long)]
        from: String,

        /// Print what would happen without making any changes.
        #[arg(long)]
        dry_run: bool,
    },

    /// Migrate the manifest schema to the current version (not yet implemented).
    #[command(hide = true)]
    Migrate,
}

// ── repo command runner ─────────────────────────────────────────────────────────────────────────

pub fn run_repo(
    command: RepoCommand,
    cwd: &Path,
    store_override: Option<&Path>,
) -> Result<ExitCode> {
    match command {
        RepoCommand::List { format, verbose } => {
            cmd_repo_list(store_override, format, verbose)?;
            Ok(ExitCode::SUCCESS)
        }
        RepoCommand::Status { format, verbose } => {
            cmd_repo_status(cwd, store_override, format, verbose)
        }
        RepoCommand::Repair { dry_run } => {
            cmd_repo_repair(cwd, store_override, dry_run)?;
            Ok(ExitCode::SUCCESS)
        }
        RepoCommand::Gc { dry_run, yes } => {
            cmd_repo_gc(cwd, store_override, dry_run, yes)?;
            Ok(ExitCode::SUCCESS)
        }
        RepoCommand::Adopt { from, dry_run } => {
            cmd_repo_adopt(cwd, store_override, &from, dry_run)?;
            Ok(ExitCode::SUCCESS)
        }
        RepoCommand::Migrate => {
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

fn cmd_repo_list(
    store_override: Option<&Path>,
    format: Option<OutputFormat>,
    verbose: bool,
) -> Result<()> {
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

    let fmt = OutputFormat::resolve(format, &config.default_format);

    match fmt {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&rows)?),
        OutputFormat::Plain => {
            for r in &rows {
                println!("{} {} {}", r.name, r.root.display(), r.item_count);
            }
        }
        OutputFormat::Table => {
            if verbose {
                // Verbose: one multi-line block per repository.
                let mut entries: Vec<(&str, &_)> = idx.iter().collect();
                entries.sort_by(|(_, a), (_, b)| {
                    let na = a
                        .root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let nb = b
                        .root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    na.cmp(&nb)
                });
                if entries.is_empty() {
                    println!("(no repositories in store)");
                    return Ok(());
                }
                for (_, entry) in &entries {
                    let repo_store = config.store.join("repos").join(&entry.store_dir);
                    let item_count = manifest::load(&repo_store)
                        .map(|m| m.items.len())
                        .unwrap_or(0);
                    let name = entry
                        .root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| entry.root.to_string_lossy().into_owned());
                    println!("  {name}");
                    println!("    root:        {}", entry.root.display());
                    println!("    git_common:  {}", entry.git_common_dir.display());
                    println!("    store_dir:   {}", entry.store_dir);
                    println!("    items:       {item_count}");
                    println!("    last_seen:   {}", entry.last_seen_at);
                    println!();
                }
            } else {
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
        }
    }
    Ok(())
}

fn cmd_repo_status(
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
    let report = ops::integrity::check(&ctx, &link, &ignore)?;

    match fmt {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&report)?),
        OutputFormat::Plain => print_repo_status_plain(&report),
        OutputFormat::Table => print_repo_status(&report, verbose, &ctx.repo_root),
    }
    Ok(classify_integrity_exit(&report))
}

fn cmd_repo_repair(cwd: &Path, store_override: Option<&Path>, dry_run: bool) -> Result<()> {
    let mut ctx =
        context::build(cwd, store_override, true).context("failed to initialise repo context")?;
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    // yes=false: safe fixes only; orphan deletion requires explicit `repo gc`.
    let report = ops::integrity::fix(&mut ctx, &link, &ignore, false, dry_run)?;

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

    // Use check() to collect orphan store items.
    let report = ops::integrity::check(&ctx, &link, &ignore)?;

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

fn cmd_repo_adopt(
    cwd: &Path,
    store_override: Option<&Path>,
    from_repo_id: &str,
    dry_run: bool,
) -> Result<()> {
    let mut ctx = context::build(cwd, store_override, !dry_run)
        .context("failed to initialise repo context")?;
    let link = SymlinkStrategy;

    let result = ops::adopt::adopt(&mut ctx, from_repo_id, dry_run, &link)?;

    if result.items.is_empty() {
        println!("no eligible items found in repository '{from_repo_id}'");
        return Ok(());
    }

    for item in &result.items {
        match item.outcome {
            AdoptOutcome::Adopted => println!("adopted:         {}", item.path),
            AdoptOutcome::AdoptedLinkFailed => {
                println!("adopted (no link): {}", item.path);
                eprintln!(
                    "warning: symlink update failed for '{}'; run 'shelfbox item repair {}' to fix",
                    item.path, item.path
                );
            }
            AdoptOutcome::Reclaimed => println!("reclaimed:       {}", item.path),
            AdoptOutcome::ReclaimedLinkFailed => {
                println!("reclaimed (no link): {}", item.path);
                eprintln!(
                    "warning: symlink update failed for '{}'; run 'shelfbox item repair {}' to fix",
                    item.path, item.path
                );
            }
            AdoptOutcome::WouldAdopt => println!("would adopt:     {}", item.path),
            AdoptOutcome::Conflict => println!("skipped (conflict):      {}", item.path),
            AdoptOutcome::StoreMissing => println!("skipped (store missing): {}", item.path),
        }
    }

    let count = result.adopted_count();
    if dry_run {
        println!("\n{count} item(s) would be adopted (dry run)");
    } else {
        println!("\n{count} item(s) adopted from '{from_repo_id}'");
    }

    Ok(())
}

// ── Human-readable formatters ───────────────────────────────────────────────────────────────────

fn print_repo_status(report: &IntegrityReport, verbose: bool, repo_root: &Path) {
    println!("repo: {}", repo_root.display());

    let total = report.items.len();
    let errors = report.items.iter().filter(|s| !s.ok).count();
    let overall = if errors > 0 { "ERROR" } else { "OK" };

    println!("items: {total} total, {errors} with issues  [{overall}]");

    for s in &report.items {
        let label = if s.ok { "OK" } else { "ERROR" };
        if verbose {
            println!("  {label:<8} {}", s.path);
            println!("    link_exists:  {}", s.link_exists);
            println!("    link_valid:   {}", s.link_valid);
            println!("    store_exists: {}", s.store_exists);
            println!("    in_exclude:   {}", s.in_exclude);
            println!("    not_tracked:  {}", s.not_tracked);
        } else {
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
            if issues.is_empty() {
                println!("  {label:<8} {}", s.path);
            } else {
                println!("  {label:<8} {}  ({})", s.path, issues.join(", "));
            }
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

fn print_repo_status_plain(report: &IntegrityReport) {
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

/// Determine the exit code for `repo status` based on the integrity report.
///
/// - 2: structural ERROR (broken/missing symlink, missing store item, git-tracked)
/// - 1: WARN only (missing exclude entry, orphan store items, root mismatch)
/// - 0: all clear
fn classify_integrity_exit(report: &IntegrityReport) -> ExitCode {
    let has_error = report
        .items
        .iter()
        .any(|s| !s.link_exists || !s.link_valid || !s.store_exists || !s.not_tracked);
    if has_error {
        return ExitCode::from(2);
    }

    let has_warn = report.items.iter().any(|s| !s.in_exclude)
        || !report.orphan_store_items.is_empty()
        || !report.repo_root_matches_index;
    if has_warn {
        return ExitCode::from(1);
    }

    ExitCode::SUCCESS
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
