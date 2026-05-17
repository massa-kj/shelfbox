use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{AppError, Result};

// ── Default store path ────────────────────────────────────────────────────────

/// Returns the default store root directory following XDG / platform conventions.
///
/// - Linux/macOS: `$XDG_DATA_HOME/shelfbox` → fallback `~/.local/share/shelfbox`
/// - Windows:     `%LOCALAPPDATA%\shelfbox`
fn default_store_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("shelfbox"))
}

// ── Raw TOML representation ───────────────────────────────────────────────────

/// Deserialisation target for `config.toml`.
///
/// All fields are optional so that a nearly-empty config file is valid.
/// Missing values fall back to platform defaults at resolution time.
#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    /// Override for the store root directory.
    store: Option<PathBuf>,
}

// ── Public resolved config ────────────────────────────────────────────────────

/// Fully resolved configuration ready for use by the rest of the library.
///
/// Construct via [`Config::load`] or [`Config::with_store_override`].
#[derive(Debug, Clone)]
pub struct Config {
    /// Root directory of the external store.
    pub store: PathBuf,
}

impl Config {
    /// Load configuration from the platform-default config path, then apply
    /// `store_override` if provided (e.g. from a `--store` CLI flag).
    ///
    /// If the config file does not exist the function silently uses defaults.
    pub fn load(store_override: Option<&Path>) -> Result<Self> {
        let raw = read_config_file()?;
        Self::resolve(raw, store_override)
    }

    fn resolve(raw: RawConfig, store_override: Option<&Path>) -> Result<Self> {
        // Priority (high → low):
        //   1. --store CLI flag (store_override)
        //   2. $SHELFBOX_STORE environment variable
        //   3. `store` key in config.toml
        //   4. XDG / platform default
        let env_store = std::env::var_os("SHELFBOX_STORE").map(PathBuf::from);
        let store = store_override
            .map(|p| p.to_path_buf())
            .or(env_store)
            .or(raw.store)
            .or_else(default_store_path)
            .ok_or_else(|| {
                AppError::Internal(
                    "could not determine store path; set `store` in config.toml or \
                     SHELFBOX_STORE env var"
                        .into(),
                )
            })?;

        Ok(Self { store })
    }

    /// Convenience constructor for tests: use an explicit store path with no
    /// config file read.
    #[cfg(test)]
    pub fn with_store(store: impl Into<PathBuf>) -> Self {
        Self {
            store: store.into(),
        }
    }
}

// ── Config file lookup ────────────────────────────────────────────────────────

/// Returns the platform-default path for the `config.toml` file.
///
/// - Linux/macOS: `$XDG_CONFIG_HOME/shelfbox/config.toml`
///   → fallback `~/.config/shelfbox/config.toml`
/// - Windows:     `%APPDATA%\shelfbox\config.toml`
pub fn config_file_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("shelfbox").join("config.toml"))
}

fn read_config_file() -> Result<RawConfig> {
    let Some(path) = config_file_path() else {
        return Ok(RawConfig::default());
    };

    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(RawConfig::default()),
        Err(e) => return Err(AppError::io(path, e)),
    };

    toml::from_str(&contents).map_err(|e| AppError::toml_parse(path, e))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Serialize any test that reads or writes SHELFBOX_STORE to avoid races
    // between threads in the test harness.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn store_override_takes_precedence_over_raw_config() {
        let _g = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("SHELFBOX_STORE");
        let raw = RawConfig {
            store: Some(PathBuf::from("/from/config")),
        };
        let override_path = PathBuf::from("/from/override");
        let cfg = Config::resolve(raw, Some(&override_path)).unwrap();
        assert_eq!(cfg.store, override_path);
    }

    #[test]
    fn raw_config_store_used_when_no_override() {
        let _g = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("SHELFBOX_STORE");
        let raw = RawConfig {
            store: Some(PathBuf::from("/from/config")),
        };
        let cfg = Config::resolve(raw, None).unwrap();
        assert_eq!(cfg.store, PathBuf::from("/from/config"));
    }

    #[test]
    fn falls_back_to_platform_default_when_both_absent() {
        let _g = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("SHELFBOX_STORE");
        let raw = RawConfig { store: None };
        // Platform default must exist on the CI runner (Linux/macOS).
        let cfg = Config::resolve(raw, None).unwrap();
        assert!(cfg.store.to_string_lossy().contains("shelfbox"));
    }

    #[test]
    fn env_var_takes_precedence_over_config_file() {
        let _g = ENV_MUTEX.lock().unwrap();
        std::env::set_var("SHELFBOX_STORE", "/from/env");
        let raw = RawConfig {
            store: Some(PathBuf::from("/from/config")),
        };
        let cfg = Config::resolve(raw, None).unwrap();
        std::env::remove_var("SHELFBOX_STORE");
        assert_eq!(cfg.store, PathBuf::from("/from/env"));
    }

    #[test]
    fn cli_flag_takes_precedence_over_env_var() {
        let _g = ENV_MUTEX.lock().unwrap();
        std::env::set_var("SHELFBOX_STORE", "/from/env");
        let raw = RawConfig {
            store: Some(PathBuf::from("/from/config")),
        };
        let override_path = PathBuf::from("/from/override");
        let cfg = Config::resolve(raw, Some(&override_path)).unwrap();
        std::env::remove_var("SHELFBOX_STORE");
        assert_eq!(cfg.store, override_path);
    }
}
