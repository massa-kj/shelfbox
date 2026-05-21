use std::path::Path;

use anyhow::Result;
use clap::Subcommand;

// ── config subcommands ──────────────────────────────────────────────────────────────────────────

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

// ── config command runner ───────────────────────────────────────────────────────────────────────

pub fn run_config(
    command: ConfigCommand,
    _cwd: &Path,
    _store_override: Option<&Path>,
) -> Result<()> {
    match command {
        ConfigCommand::Get { .. }
        | ConfigCommand::Path
        | ConfigCommand::Set { .. }
        | ConfigCommand::Edit => anyhow::bail!("not yet implemented"),
    }
}
