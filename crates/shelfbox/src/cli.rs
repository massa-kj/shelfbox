use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

use crate::cmd::{
    config::{run_config, ConfigCommand},
    internal::{run_internal, InternalCommand},
    item::{run_item, ItemCommand},
    repo::{run_repo, RepoCommand},
    store::{run_store, StoreCommand},
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

#[derive(Debug, clap::Subcommand)]
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

// ── Entry point ─────────────────────────────────────────────────────────────────────────────────

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir().context("failed to get current directory")?;
    let store_override = cli.store.as_deref();

    match cli.command {
        Command::Item { command } => run_item(command, &cwd, store_override),
        Command::Repo { command } => run_repo(command, &cwd, store_override),
        Command::Store { command } => run_store(command, &cwd, store_override),
        Command::Config { command } => run_config(command, &cwd, store_override),
        Command::Internal { command } => run_internal(command, &cwd, store_override),
    }
}
