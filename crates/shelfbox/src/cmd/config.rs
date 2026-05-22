use std::path::Path;

use anyhow::Result;
use clap::Subcommand;
use shelfbox_core::config::{config_file_path, Config};

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
    store_override: Option<&Path>,
) -> Result<()> {
    match command {
        ConfigCommand::Get { key } => cmd_config_get(&key, store_override),
        ConfigCommand::Path => cmd_config_path(),
        ConfigCommand::Set { .. } | ConfigCommand::Edit => {
            anyhow::bail!("not yet implemented")
        }
    }
}

// ── subcommand implementations ─────────────────────────────────────────────────────────────────

fn cmd_config_path() -> Result<()> {
    match config_file_path() {
        Some(p) => println!("{}", p.display()),
        None => eprintln!("could not determine config file path"),
    }
    Ok(())
}

fn cmd_config_get(key: &str, store_override: Option<&Path>) -> Result<()> {
    let cfg = Config::load(store_override)?;
    match key {
        "store" => println!("{}", cfg.store.display()),
        other => anyhow::bail!("unknown config key: {other}"),
    }
    Ok(())
}
