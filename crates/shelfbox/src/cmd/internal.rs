use std::io;
use std::path::Path;

use anyhow::Result;
use clap::CommandFactory;
use clap::Subcommand;
use clap_complete::Shell;
use shelfbox_core::{config::Config, context, store::index};

// ── internal subcommands ────────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum InternalCommand {
    /// Dump internal state for debugging.
    Debug,

    /// Output shell completion script.
    Completions {
        /// Target shell (bash, zsh, fish, elvish, powershell).
        #[arg(value_name = "SHELL", value_enum)]
        shell: Shell,
    },
}

// ── internal command runner ─────────────────────────────────────────────────────────────────────

pub fn run_internal(
    command: InternalCommand,
    cwd: &Path,
    store_override: Option<&Path>,
) -> Result<()> {
    match command {
        InternalCommand::Debug => cmd_debug(cwd, store_override),
        InternalCommand::Completions { shell } => cmd_completions(shell),
    }
}

// ── subcommand implementations ─────────────────────────────────────────────────────────────────

fn cmd_debug(cwd: &Path, store_override: Option<&Path>) -> Result<()> {
    let cfg = Config::load(store_override)?;
    println!("[config]");
    println!("  store = {}", cfg.store.display());

    let idx = index::load(&cfg.store)?;
    println!("\n[index]");
    let mut entries: Vec<_> = idx.iter().collect();
    entries.sort_by_key(|(_, e)| e.last_seen_at.as_str());
    for (id, entry) in &entries {
        println!("  {id}");
        println!("    root        = {}", entry.root.display());
        println!("    store_dir   = {}", entry.store_dir);
        println!("    last_seen   = {}", entry.last_seen_at);
    }

    // Attempt to build repo context if we are inside a git repo.
    println!("\n[context]");
    match context::build(cwd, store_override, false) {
        Ok(ctx) => {
            println!("  repo_id     = {}", ctx.repo_id);
            println!("  repo_root   = {}", ctx.repo_root.display());
            println!("  repo_store  = {}", ctx.repo_store.display());
        }
        Err(e) => println!("  (not in a git repo or context error: {e})"),
    }

    Ok(())
}

fn cmd_completions(shell: Shell) -> Result<()> {
    let mut cmd = crate::cli::Cli::command();
    clap_complete::generate(shell, &mut cmd, "shelfbox", &mut io::stdout());
    Ok(())
}
