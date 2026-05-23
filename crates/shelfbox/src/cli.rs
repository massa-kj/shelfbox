use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

use crate::cmd::{
    config::{run_config, ConfigCommand},
    format::OutputFormat,
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

    /// Hidden alias for `repo status` (follows the `brew doctor` / `flutter doctor` convention).
    ///
    /// Runs a read-only health check for the current repository's shelf.
    /// For the canonical command and full documentation, see `shelfbox repo status --help`.
    #[command(hide = true)]
    Doctor {
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
}

// ── Entry point ─────────────────────────────────────────────────────────────────────────────────

pub fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir().context("failed to get current directory")?;
    let store_override = cli.store.as_deref();

    match cli.command {
        Command::Item { command } => {
            run_item(command, &cwd, store_override)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Repo { command } => run_repo(command, &cwd, store_override),
        Command::Store { command } => run_store(command, &cwd, store_override),
        Command::Config { command } => {
            run_config(command, &cwd, store_override)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Internal { command } => {
            run_internal(command, &cwd, store_override)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Doctor { format } => {
            run_repo(RepoCommand::Status { format }, &cwd, store_override)
        }
    }
}
