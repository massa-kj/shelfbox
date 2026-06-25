use std::path::Path;

use anyhow::Result;
use clap::Subcommand;
use serde::Serialize;
use shelfbox_core::api::config;

use crate::commands::format::OutputFormat;

// ── Static key metadata ─────────────────────────────────────────────────────────────────────────

struct KeyMeta {
    key: &'static str,
    type_name: &'static str,
    default_display: &'static str,
    description: &'static str,
    precedence: &'static [&'static str],
}

const KEY_STORE: KeyMeta = KeyMeta {
    key: "store",
    type_name: "path",
    default_display: "~/.local/share/shelfbox",
    description: "Root directory for the global shelfbox store.",
    precedence: &["--store", "SHELFBOX_STORE", "config.toml", "XDG default"],
};

const KEY_DEFAULT_FORMAT: KeyMeta = KeyMeta {
    key: "default_format",
    type_name: "enum",
    default_display: "table",
    description: "Default output format for list/status commands. \
                  Valid values: table, plain, json.",
    precedence: &["config.toml", "built-in default"],
};

const ALL_KEYS: &[&KeyMeta] = &[&KEY_STORE, &KEY_DEFAULT_FORMAT];

fn find_key(key: &str) -> Option<&'static KeyMeta> {
    ALL_KEYS.iter().copied().find(|m| m.key == key)
}

// ── config subcommands ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the value of a configuration key.
    Get {
        #[arg(value_name = "KEY")]
        key: String,
        /// Also print the origin of the value (cli, env, config, default).
        #[arg(long)]
        source: bool,
    },

    /// List all configuration keys with their current values and sources.
    List {
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },

    /// Show detailed information about a configuration key.
    Explain {
        #[arg(value_name = "KEY")]
        key: String,
    },

    /// Print the path to the configuration file.
    Path,

    /// Set the value of a configuration key.
    Set {
        #[arg(value_name = "KEY")]
        key: String,

        #[arg(value_name = "VALUE")]
        value: String,
    },
}

// ── config command runner ───────────────────────────────────────────────────────────────────────

pub fn run_config(
    command: ConfigCommand,
    _cwd: &Path,
    store_override: Option<&Path>,
) -> Result<()> {
    match command {
        ConfigCommand::Get { key, source } => cmd_config_get(&key, source, store_override),
        ConfigCommand::List { format } => cmd_config_list(format, store_override),
        ConfigCommand::Explain { key } => cmd_config_explain(&key),
        ConfigCommand::Path => cmd_config_path(),
        ConfigCommand::Set { key, value } => cmd_config_set(&key, &value),
    }
}

// ── subcommand implementations ─────────────────────────────────────────────────────────────────

fn cmd_config_path() -> Result<()> {
    match config::config_file_path() {
        Some(p) => println!("{}", p.display()),
        None => eprintln!("could not determine config file path"),
    }
    Ok(())
}

fn cmd_config_get(key: &str, show_source: bool, store_override: Option<&Path>) -> Result<()> {
    let _ = find_key(key).ok_or_else(|| anyhow::anyhow!("unknown config key: {key}"))?;
    let resolved = config::load_resolved(store_override)?;

    let (value, source) = match key {
        "store" => (resolved.store.display().to_string(), resolved.store_source),
        "default_format" => (
            resolved
                .default_format
                .unwrap_or_else(|| KEY_DEFAULT_FORMAT.default_display.to_string()),
            resolved.default_format_source,
        ),
        _ => unreachable!(),
    };

    println!("{value}");
    if show_source {
        println!("source: {source}");
    }
    Ok(())
}

fn cmd_config_list(format: OutputFormat, store_override: Option<&Path>) -> Result<()> {
    let resolved = config::load_resolved(store_override)?;

    struct Row {
        key: &'static str,
        type_name: &'static str,
        default: &'static str,
        source: String,
        current: String,
    }

    let rows = vec![
        Row {
            key: KEY_STORE.key,
            type_name: KEY_STORE.type_name,
            default: KEY_STORE.default_display,
            source: resolved.store_source.short().to_string(),
            current: resolved.store.display().to_string(),
        },
        Row {
            key: KEY_DEFAULT_FORMAT.key,
            type_name: KEY_DEFAULT_FORMAT.type_name,
            default: KEY_DEFAULT_FORMAT.default_display,
            source: resolved.default_format_source.short().to_string(),
            current: resolved
                .default_format
                .unwrap_or_else(|| KEY_DEFAULT_FORMAT.default_display.to_string()),
        },
    ];

    match format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            struct Entry {
                key: &'static str,
                #[serde(rename = "type")]
                type_name: &'static str,
                default: &'static str,
                source: String,
                current: String,
            }
            let entries: Vec<Entry> = rows
                .into_iter()
                .map(|r| Entry {
                    key: r.key,
                    type_name: r.type_name,
                    default: r.default,
                    source: r.source,
                    current: r.current,
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&entries)?);
        }
        _ => {
            let kw = rows.iter().map(|r| r.key.len()).max().unwrap_or(0).max(3);
            let tw = rows
                .iter()
                .map(|r| r.type_name.len())
                .max()
                .unwrap_or(0)
                .max(4);
            let dw = rows
                .iter()
                .map(|r| r.default.len())
                .max()
                .unwrap_or(0)
                .max(7);
            let sw = rows
                .iter()
                .map(|r| r.source.len())
                .max()
                .unwrap_or(0)
                .max(6);
            println!(
                "{}  {}  {}  {}  CURRENT",
                ljust("KEY", kw),
                ljust("TYPE", tw),
                ljust("DEFAULT", dw),
                ljust("SOURCE", sw),
            );
            for r in &rows {
                println!(
                    "{}  {}  {}  {}  {}",
                    ljust(r.key, kw),
                    ljust(r.type_name, tw),
                    ljust(r.default, dw),
                    ljust(&r.source, sw),
                    r.current,
                );
            }
        }
    }
    Ok(())
}

fn cmd_config_explain(key: &str) -> Result<()> {
    let meta = find_key(key).ok_or_else(|| anyhow::anyhow!("unknown config key: {key}"))?;
    println!("KEY: {}", meta.key);
    println!("TYPE: {}", meta.type_name);
    println!("DEFAULT:");
    println!("  {}", meta.default_display);
    println!();
    println!("DESCRIPTION:");
    println!("  {}", meta.description);
    println!();
    println!("PRECEDENCE:");
    for step in meta.precedence {
        println!("  {step}");
    }
    Ok(())
}

fn cmd_config_set(key: &str, value: &str) -> Result<()> {
    config::set_key(key, value)?;
    println!("set {key} = {value}");
    Ok(())
}

fn ljust(s: &str, w: usize) -> String {
    format!("{s:<w$}", s = s, w = w)
}
