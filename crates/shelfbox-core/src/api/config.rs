use std::path::Path;

pub use crate::config::{Config, ConfigSource, ResolvedConfig};

use crate::{config, error::Result};

pub fn load(store_override: Option<&Path>) -> Result<Config> {
    Config::load(store_override)
}

pub fn load_resolved(store_override: Option<&Path>) -> Result<ResolvedConfig> {
    Config::load_resolved(store_override)
}

pub fn config_file_path() -> Option<std::path::PathBuf> {
    config::config_file_path()
}

pub fn set_key(key: &str, value: &str) -> Result<()> {
    config::set_key(key, value)
}
