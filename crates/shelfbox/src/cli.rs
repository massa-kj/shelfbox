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
    /// Manage individual shelved items.
    Item {
        #[command(subcommand)]
        command: ItemCommand,
    },
    /// Manage the current repository's shelf.
    Repo {
        #[command(subcommand)]
        command: RepoCommand,
    },
    /// Manage the global store.
    Store {
        #[command(subcommand)]
        command: StoreCommand,
    },
    /// Manage shelfbox configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Internal and development commands.
    #[command(hide = true)]
    Internal {
        #[command(subcommand)]
        command: InternalCommand,
    },
}

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

    /// Rename a shelved item's tracked path (not yet implemented).
    Move {
        #[arg(value_name = "OLD")]
        old: PathBuf,

        #[arg(value_name = "NEW")]
        new_path: PathBuf,
    },

    /// Show metadata for a shelved item (not yet implemented).
    Info {
        #[arg(value_name = "PATH")]
        path: PathBuf,
    },
}

// ── repo subcommands ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum RepoCommand {
    /// List all repositories known to the store.
    List,

    /// Show the health status of the current repository's shelf.
    Status,

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

// ── store subcommands ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum StoreCommand {
    /// Show store metadata (path, repo count, disk usage).
    Info,

    /// Run a deep integrity check across all store contents.
    Verify,

    /// Delete store entries for repositories that no longer exist.
    Gc {
        /// Print what would be deleted without making any changes.
        #[arg(long)]
        dry_run: bool,

        /// Skip confirmation prompt and perform deletions immediately.
        #[arg(long)]
        yes: bool,
    },
}

// ── config subcommands ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the value of a configuration key.
    Get {
        #[arg(value_name = "KEY")]
        key: String,
    },

    /// Print the path to the configuration file.
    Path,

    /// Set the value of a configuration key (not yet implemented).
    Set {
        #[arg(value_name = "KEY")]
        key: String,

        #[arg(value_name = "VALUE")]
        value: String,
    },

    /// Open the configuration file in $EDITOR (not yet implemented).
    Edit,
}

// ── internal subcommands ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum InternalCommand {
    /// Dump internal state for debugging.
    Debug,

    /// Output shell completion script.
    Completions {
        /// Target shell (bash, zsh, fish).
        #[arg(value_name = "SHELL")]
        shell: String,
    },
}

// ── Entry point ────────────────────────────────────────────────────────────────

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir().context("failed to get current directory")?;
    let store_override = cli.store.as_deref();

    match cli.command {
        Command::Item { command } => run_item(command, &cwd, store_override),
        Command::Repo { command } => run_repo(command, &cwd, store_override),
        Command::Store { command } => run_store(command, &cwd, store_override),
        Command::Config { command } => run_config(command, &cwd, store_override),
        Command::Internal { command } => run_internal(command),
    }
}

fn run_item(command: ItemCommand, cwd: &Path, store_override: Option<&Path>) -> Result<()> {
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
        ItemCommand::List { json } => cmd_list(cwd, store_override, json),
        ItemCommand::Status { json } => cmd_status(cwd, store_override, json),
        ItemCommand::Move { .. } | ItemCommand::Info { .. } => {
            anyhow::bail!("not yet implemented")
        }
    }
}

fn run_repo(command: RepoCommand, _cwd: &Path, _store_override: Option<&Path>) -> Result<()> {
    match command {
        RepoCommand::List
        | RepoCommand::Status
        | RepoCommand::Repair { .. }
        | RepoCommand::Gc { .. }
        | RepoCommand::Relink
        | RepoCommand::Migrate => anyhow::bail!("not yet implemented"),
    }
}

fn run_store(command: StoreCommand, _cwd: &Path, _store_override: Option<&Path>) -> Result<()> {
    match command {
        StoreCommand::Info | StoreCommand::Verify | StoreCommand::Gc { .. } => {
            anyhow::bail!("not yet implemented")
        }
    }
}

fn run_config(command: ConfigCommand, _cwd: &Path, _store_override: Option<&Path>) -> Result<()> {
    match command {
        ConfigCommand::Get { .. }
        | ConfigCommand::Path
        | ConfigCommand::Set { .. }
        | ConfigCommand::Edit => anyhow::bail!("not yet implemented"),
    }
}

fn run_internal(command: InternalCommand) -> Result<()> {
    match command {
        InternalCommand::Debug | InternalCommand::Completions { .. } => {
            anyhow::bail!("not yet implemented")
        }
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
    keep_store: bool,
) -> Result<()> {
    if keep_store {
        anyhow::bail!("--keep-store is not yet implemented");
    }
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
