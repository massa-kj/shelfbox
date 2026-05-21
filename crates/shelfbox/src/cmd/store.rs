use std::path::Path;

use anyhow::Result;
use clap::Subcommand;

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

// ── store command runner ────────────────────────────────────────────────────────────────────────

pub fn run_store(command: StoreCommand, _cwd: &Path, _store_override: Option<&Path>) -> Result<()> {
    match command {
        StoreCommand::Info | StoreCommand::Verify | StoreCommand::Gc { .. } => {
            anyhow::bail!("not yet implemented")
        }
    }
}
