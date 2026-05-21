use std::path::Path;

use anyhow::Result;
use clap::Subcommand;

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

// ── repo command runner ─────────────────────────────────────────────────────────────────────────

pub fn run_repo(command: RepoCommand, _cwd: &Path, _store_override: Option<&Path>) -> Result<()> {
    match command {
        RepoCommand::List
        | RepoCommand::Status
        | RepoCommand::Repair { .. }
        | RepoCommand::Gc { .. }
        | RepoCommand::Relink
        | RepoCommand::Migrate => anyhow::bail!("not yet implemented"),
    }
}
