use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use shelfbox_core::{
    context,
    error::AppError,
    ignore::GitInfoExclude,
    link::SymlinkStrategy,
    ops,
    ops::status::ItemStatus,
    store::manifest::{Item, ItemKind},
};

/// Shelve repo-local files outside Git, keeping them visible in your editor.
#[derive(Debug, Parser)]
#[command(name = "shelfbox", version, about)]
pub struct Cli {
    /// Override the store directory (takes precedence over config).
    #[arg(long, global = true, value_name = "PATH")]
    pub store: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
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
    },

    /// List all shelved files for the current repository.
    List {
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Show the health status of each shelved file.
    Status {
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Check for broken links, missing store items, and other inconsistencies.
    Doctor {
        /// Apply safe automatic fixes for detected issues.
        #[arg(long)]
        fix: bool,

        /// When used with --fix, also perform potentially destructive actions
        /// such as deleting orphan store items without prompting.
        #[arg(long, requires = "fix")]
        yes: bool,

        /// Output as JSON.
        #[arg(long)]
        json: bool,
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
}

// ── Entry point ────────────────────────────────────────────────────────────────

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir().context("failed to get current directory")?;
    let store_override = cli.store.as_deref();

    match cli.command {
        Command::Add { paths, dry_run } => cmd_add(&cwd, store_override, &paths, dry_run),
        Command::Restore {
            paths,
            dry_run,
            keep_ignore,
        } => cmd_restore(&cwd, store_override, &paths, dry_run, keep_ignore),
        Command::List { json } => cmd_list(&cwd, store_override, json),
        Command::Status { json } => cmd_status(&cwd, store_override, json),
        Command::Doctor { fix, yes, json } => cmd_doctor(&cwd, store_override, fix, yes, json),
        Command::Repair { paths, dry_run } => cmd_repair(&cwd, store_override, &paths, dry_run),
    }
}

// ── Subcommand handlers ────────────────────────────────────────────────────────

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
) -> Result<()> {
    let mut ctx =
        context::build(cwd, store_override, true).context("failed to initialise repo context")?;
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    for path in paths {
        let abs = resolve_path(cwd, path);
        ops::restore::restore(&mut ctx, &abs, dry_run, keep_ignore, &link, &ignore)
            .with_context(|| format!("restore '{}' failed", path.display()))?;
        if !dry_run {
            println!("restored: {}", path.display());
        }
    }
    Ok(())
}

fn cmd_list(cwd: &Path, store_override: Option<&Path>, json: bool) -> Result<()> {
    let ctx =
        context::build(cwd, store_override, false).context("failed to initialise repo context")?;
    let items = ops::list::list(&ctx);

    if json {
        println!("{}", serde_json::to_string_pretty(items)?);
    } else {
        print_list(items);
    }
    Ok(())
}

fn cmd_status(cwd: &Path, store_override: Option<&Path>, json: bool) -> Result<()> {
    let ctx =
        context::build(cwd, store_override, false).context("failed to initialise repo context")?;
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;
    let statuses = ops::status::status(&ctx, &link, &ignore)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&statuses)?);
    } else {
        print_status(&statuses);
    }
    Ok(())
}

fn cmd_doctor(
    cwd: &Path,
    store_override: Option<&Path>,
    fix: bool,
    yes: bool,
    json: bool,
) -> Result<()> {
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    if fix {
        let mut ctx = context::build(cwd, store_override, true)
            .context("failed to initialise repo context")?;
        let report = ops::doctor::doctor_fix(&mut ctx, &link, &ignore, yes, false)?;
        if json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print_fix_report(&report);
        }
    } else {
        let ctx = context::build(cwd, store_override, false)
            .context("failed to initialise repo context")?;
        let report = ops::doctor::doctor(&ctx, &link, &ignore)?;
        if json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print_doctor_report(
                &report.items,
                &report.orphan_store_items,
                report.repo_root_matches_index,
            );
        }
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

// ── Human-readable formatters ──────────────────────────────────────────────────

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

fn print_doctor_report(statuses: &[ItemStatus], orphans: &[String], root_matches: bool) {
    // Repo root integrity line.
    if root_matches {
        println!("{:<8} repo root matches index", "OK");
    } else {
        println!(
            "{:<8} repo root mismatch: repository may have been moved",
            "ERROR"
        );
        println!("  → Run: shelfbox doctor --fix");
    }

    // Print each item with a navigation hint for non-OK states.
    if statuses.is_empty() {
        println!("(no shelved items)");
    } else {
        for s in statuses {
            let (label, issues) = classify_status(s);
            if issues.is_empty() {
                println!("{:<8} {}", label, s.path);
            } else {
                println!("{:<8} {}  ({})", label, s.path, issues.join(", "));
                // Show the most actionable next step for this item.
                if !s.store_exists {
                    println!("  → Data loss: cannot auto-repair. Restore manually and re-add.");
                } else if !s.link_exists || !s.link_valid {
                    println!("  → Run: shelfbox repair {}", s.path);
                } else {
                    println!("  → Run: shelfbox doctor --fix");
                }
            }
        }
    }

    if !orphans.is_empty() {
        if !statuses.is_empty() {
            println!();
        }
        println!("--- orphan store items (not in manifest) ---");
        for orphan in orphans {
            println!("  WARN     orphan: {orphan}");
        }
        println!("  → Run: shelfbox doctor --fix");
    }
}

fn print_fix_report(report: &ops::doctor::DoctorFixReport) {
    use ops::doctor::FixResult;

    // Show "everything is healthy" when there are no actions at all, or when
    // every action is a Skipped (nothing needed fixing).
    let all_skipped = !report.actions.is_empty()
        && report
            .actions
            .iter()
            .all(|a| matches!(a, FixResult::Skipped(_)));
    if report.actions.is_empty() || all_skipped {
        println!("{:<12} everything is healthy", "OK");
        return;
    }

    for action in &report.actions {
        match action {
            FixResult::Fixed(msg) => println!("{:<12} {msg}", "FIXED"),
            FixResult::Skipped(msg) => println!("{:<12} {msg}", "OK"),
            FixResult::Failed(msg) => println!("{:<12} {msg}", "ERROR"),
            FixResult::NeedsConfirmation(msg) => println!("{:<12} {msg}", "CONFIRM"),
            FixResult::CannotFix(msg) => println!("{:<12} {msg}", "WARN"),
        }
    }

    if !report.data_loss_warnings.is_empty() {
        println!();
        println!("--- data loss warnings ---");
        for w in &report.data_loss_warnings {
            println!("  {w}");
            println!(
                "  Restore manually: rm <symlink> && cp <backup> <path> && shelfbox add <path>"
            );
        }
    }
}

/// Returns `(severity_label, list_of_problem_descriptions)` for one item.
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
        issues.push("tracked by git");
    }

    let label = if !s.link_exists || !s.link_valid || !s.store_exists {
        "ERROR"
    } else if !issues.is_empty() {
        "WARN"
    } else {
        "OK"
    };

    (label, issues)
}

// ── Path helpers ───────────────────────────────────────────────────────────────

/// Resolves `path` to an absolute path without following symlinks.
///
/// - Absolute paths are returned as-is (after normalisation).
/// - Relative paths are resolved against `cwd`.
///
/// `.` and `..` components are collapsed lexically so the result matches
/// what other parts of the code expect when comparing against `repo_root`.
fn resolve_path(cwd: &Path, path: &Path) -> PathBuf {
    let base = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    normalize_path(&base)
}

/// Collapses `.` and `..` components without touching the filesystem.
fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}
