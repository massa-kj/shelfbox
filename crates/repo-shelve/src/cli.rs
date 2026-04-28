use anyhow::Result;
use clap::{Parser, Subcommand};

/// Shelve repo-local files outside Git, keeping them visible in your editor.
#[derive(Debug, Parser)]
#[command(name = "repo-shelve", version, about)]
pub struct Cli {
    /// Override the store directory (takes precedence over config).
    #[arg(long, global = true, value_name = "PATH")]
    pub store: Option<std::path::PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Move a file into the store and leave a symlink in its place.
    Add {
        /// Files to shelve (relative to repo root).
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<std::path::PathBuf>,

        /// Print what would happen without making any changes.
        #[arg(long)]
        dry_run: bool,
    },

    /// Return a shelved file to its original location and remove it from the store.
    Restore {
        /// Files to restore (relative to repo root).
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<std::path::PathBuf>,

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

pub fn run() -> Result<()> {
    let _cli = Cli::parse();
    // Dispatch to ops is implemented in Phase 6.
    // Each subcommand passes plain Rust types (PathBuf / bool) to core ops;
    // clap types never leak into repo-shelve-core.
    anyhow::bail!("not yet implemented — see plan-mvp.md")
}
