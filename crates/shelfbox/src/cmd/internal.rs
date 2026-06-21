use std::io;
use std::path::Path;

use anyhow::Result;
use clap::CommandFactory;
use clap::Subcommand;
use clap_complete::Shell;
use shelfbox_core::api::{config, repo};

// ── internal subcommands ────────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum InternalCommand {
    /// Dump internal state for debugging.
    ///
    /// By default, the home directory prefix in all paths is replaced with `~`
    /// to reduce the risk of leaking absolute paths in AI pastes or public issues.
    /// Use `--allow-sensitive` to print raw paths.
    Debug {
        /// Print raw absolute paths instead of masking the home directory with `~`.
        #[arg(long)]
        allow_sensitive: bool,
    },

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
        InternalCommand::Debug { allow_sensitive } => {
            cmd_debug(cwd, store_override, allow_sensitive)
        }
        InternalCommand::Completions { shell } => cmd_completions(shell),
    }
}

// ── subcommand implementations ─────────────────────────────────────────────────────────────────

/// Mask the home directory prefix in `path` with `~`.
/// Returns the original path unchanged if `dirs::home_dir()` is unavailable
/// or the path does not start with the home directory.
fn mask_home(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rel) = path.strip_prefix(&home) {
            return format!("~/{}", rel.display());
        }
    }
    path.display().to_string()
}

/// Format `path` for display, optionally masking the home directory.
fn display_path(path: &Path, allow_sensitive: bool) -> String {
    if allow_sensitive {
        path.display().to_string()
    } else {
        mask_home(path)
    }
}

fn cmd_debug(cwd: &Path, store_override: Option<&Path>, allow_sensitive: bool) -> Result<()> {
    let cfg = config::load(store_override)?;
    println!("[config]");
    println!("  store = {}", display_path(&cfg.store, allow_sensitive));

    let idx = repo::load_index(&cfg.store)?;
    println!("\n[index]");
    let mut entries: Vec<_> = idx.iter().collect();
    entries.sort_by_key(|(_, e)| e.last_seen_at.as_str());
    for (id, entry) in &entries {
        println!("  {id}");
        let root = entry
            .root
            .as_ref()
            .map(|root| display_path(root, allow_sensitive))
            .unwrap_or_else(|| "(unassociated)".to_string());
        println!("    root        = {root}");
        println!("    store_dir   = {}", entry.repo_store_dir);
        println!("    last_seen   = {}", entry.last_seen_at);
    }

    // Attempt to build repo context if we are inside a git repo.
    println!("\n[context]");
    match repo::build_read_only(cwd, store_override) {
        Ok(read_only) => {
            let Some(ctx) = read_only.repo else {
                println!(
                    "  repo_root   = {}",
                    display_path(&read_only.current.repo_root, allow_sensitive)
                );
                println!("  (not associated with this store)");
                return Ok(());
            };
            println!("  repo_id     = {}", ctx.repo_id);
            println!(
                "  repo_root   = {}",
                display_path(&ctx.repo_root, allow_sensitive)
            );
            println!(
                "  repo_store  = {}",
                display_path(&ctx.repo_store, allow_sensitive)
            );
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
