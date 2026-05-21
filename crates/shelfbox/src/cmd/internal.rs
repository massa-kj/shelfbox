use anyhow::Result;
use clap::Subcommand;

// ── internal subcommands ────────────────────────────────────────────────────────────────────────

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

// ── internal command runner ─────────────────────────────────────────────────────────────────────

pub fn run_internal(command: InternalCommand) -> Result<()> {
    match command {
        InternalCommand::Debug | InternalCommand::Completions { .. } => {
            anyhow::bail!("not yet implemented")
        }
    }
}
