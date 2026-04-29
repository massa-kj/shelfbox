use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use repo_shelve_core::{
    context,
    ignore::GitInfoExclude,
    link::SymlinkStrategy,
    ops,
    ops::status::ItemStatus,
    store::manifest::{Item, ItemKind},
};

/// Shelve repo-local files outside Git, keeping them visible in your editor.
#[derive(Debug, Parser)]
#[command(name = "repo-shelve", version, about)]
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
        /// Output as JSON.
        #[arg(long)]
        json: bool,
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
        Command::Doctor { json } => cmd_doctor(&cwd, store_override, json),
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
        context::build(cwd, store_override).context("failed to initialise repo context")?;
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;

    for path in paths {
        let abs = resolve_path(cwd, path);
        ops::add::add(&mut ctx, &abs, dry_run, &link, &ignore)
            .with_context(|| format!("add '{}' failed", path.display()))?;
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
        context::build(cwd, store_override).context("failed to initialise repo context")?;
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
    let ctx = context::build(cwd, store_override).context("failed to initialise repo context")?;
    let items = ops::list::list(&ctx);

    if json {
        println!("{}", serde_json::to_string_pretty(items)?);
    } else {
        print_list(items);
    }
    Ok(())
}

fn cmd_status(cwd: &Path, store_override: Option<&Path>, json: bool) -> Result<()> {
    let ctx = context::build(cwd, store_override).context("failed to initialise repo context")?;
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

fn cmd_doctor(cwd: &Path, store_override: Option<&Path>, json: bool) -> Result<()> {
    let ctx = context::build(cwd, store_override).context("failed to initialise repo context")?;
    let link = SymlinkStrategy;
    let ignore = GitInfoExclude;
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
    }

    print_status(statuses);

    if !orphans.is_empty() {
        if !statuses.is_empty() {
            println!();
        }
        println!("--- orphan store items (not in manifest) ---");
        for orphan in orphans {
            println!("  WARN     orphan: {orphan}");
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
