use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{
    domain::materialization::MaterializationStrategy,
    domain::mutation_durability::MutationDurability,
    error::{AppError, Result},
    storage::atomic_write::{self, ParentDirMode},
};

// ── Default store path ────────────────────────────────────────────────────────

/// Returns the default store root directory following XDG / platform conventions.
///
/// - `$XDG_DATA_HOME/shelfbox`, when `XDG_DATA_HOME` is an absolute path
/// - Otherwise, the platform data-local directory plus `shelfbox`
fn default_store_path() -> Option<PathBuf> {
    xdg_dir("XDG_DATA_HOME")
        .or_else(dirs::data_local_dir)
        .map(|d| d.join("shelfbox"))
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
    /// Default output format ("table" | "plain" | "json").
    default_format: Option<String>,
    /// Default strategy for materializations created in the future.
    materialization: Option<MaterializationStrategy>,
    /// Durability contract for shelf mutations on this machine.
    mutation_durability: Option<MutationDurability>,
}

// ── Config source metadata ──────────────────────────────────────────────────

/// The origin of a resolved configuration value, ordered by precedence
/// (highest to lowest).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    /// Passed via the `--store` CLI flag.
    CliFlag,
    /// Read from an environment variable; carries the variable name.
    Env(&'static str),
    /// Read from `config.toml`.
    File,
    /// Computed from the platform default.
    Default,
}

impl ConfigSource {
    /// Short label for tabular output (e.g. `config list` SOURCE column).
    /// `Env("SHELFBOX_STORE")` → `"env"`, others unchanged from
    /// [`std::fmt::Display`].
    pub fn short(self) -> &'static str {
        match self {
            Self::CliFlag => "cli",
            Self::Env(_) => "env",
            Self::File => "config",
            Self::Default => "default",
        }
    }
}

impl fmt::Display for ConfigSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CliFlag => write!(f, "cli"),
            Self::Env(var) => write!(f, "env:{var}"),
            Self::File => write!(f, "config"),
            Self::Default => write!(f, "default"),
        }
    }
}

/// Fully resolved configuration together with the origin of each value.
///
/// Used by `config list` / `config get --source` to show where values come
/// from.  For pure business-logic use, prefer the lighter [`Config`].
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    /// Resolved store root directory.
    pub store: PathBuf,
    /// Origin of [`store`](Self::store).
    pub store_source: ConfigSource,
    /// Resolved default output format, or `None` if using the built-in
    /// default (`"table"`).
    pub default_format: Option<String>,
    /// Origin of [`default_format`](Self::default_format).
    pub default_format_source: ConfigSource,
    /// Resolved default strategy for newly created materializations.
    pub materialization: MaterializationStrategy,
    /// Origin of [`materialization`](Self::materialization).
    pub materialization_source: ConfigSource,
    /// Resolved durability contract for shelf mutations.
    pub mutation_durability: MutationDurability,
    /// Origin of [`mutation_durability`](Self::mutation_durability).
    pub mutation_durability_source: ConfigSource,
}

impl From<ResolvedConfig> for Config {
    fn from(r: ResolvedConfig) -> Self {
        Self {
            store: r.store,
            default_format: r.default_format,
            materialization: r.materialization,
            mutation_durability: r.mutation_durability,
        }
    }
}

// ── Public resolved config ────────────────────────────────────────────────────

/// Fully resolved configuration ready for use by the rest of the library.
///
/// Construct via [`Config::load`].
#[derive(Debug, Clone)]
pub struct Config {
    /// Root directory of the external store.
    pub store: PathBuf,
    /// Default output format ("table" | "plain" | "json"), or `None` to use
    /// the built-in default (`"table"`).
    pub default_format: Option<String>,
    /// Default strategy for future materializations.
    ///
    /// Existing paths must always be inspected; changing this value never
    /// converts a materialization already present in a repository.
    pub materialization: MaterializationStrategy,
    /// Parent-directory durability contract for shelf mutations. This is
    /// local configuration and never changes an existing materialization.
    pub mutation_durability: MutationDurability,
}

impl Config {
    /// Load configuration from the platform-default config path, then apply
    /// `store_override` if provided (e.g. from a `--store` CLI flag).
    ///
    /// If the config file does not exist the function silently uses defaults.
    pub fn load(store_override: Option<&Path>) -> Result<Self> {
        Ok(Self::load_resolved(store_override)?.into())
    }

    /// Like [`load`](Self::load), but also returns the origin of each value.
    ///
    /// Use this when you need to display *where* a value came from
    /// (e.g. `config list`, `config get --source`).
    pub fn load_resolved(store_override: Option<&Path>) -> Result<ResolvedConfig> {
        let raw = read_config_file()?;
        Self::resolve_full(raw, store_override)
    }

    fn resolve_full(raw: RawConfig, store_override: Option<&Path>) -> Result<ResolvedConfig> {
        // Priority (high → low):
        //   1. --store CLI flag (store_override)
        //   2. $SHELFBOX_STORE environment variable
        //   3. `store` key in config.toml
        //   4. XDG / platform default
        let env_store = std::env::var_os("SHELFBOX_STORE").map(PathBuf::from);

        let (store, store_source) = if let Some(p) = store_override {
            (p.to_path_buf(), ConfigSource::CliFlag)
        } else if let Some(p) = env_store {
            (p, ConfigSource::Env("SHELFBOX_STORE"))
        } else if let Some(p) = raw.store {
            (p, ConfigSource::File)
        } else if let Some(p) = default_store_path() {
            (p, ConfigSource::Default)
        } else {
            return Err(AppError::Internal(
                "could not determine store path; set `store` in config.toml or \
                 SHELFBOX_STORE env var"
                    .into(),
            ));
        };

        let (default_format, default_format_source) = if let Some(f) = raw.default_format {
            (Some(f), ConfigSource::File)
        } else {
            (None, ConfigSource::Default)
        };

        let (materialization, materialization_source) = match raw.materialization {
            Some(strategy) => (strategy, ConfigSource::File),
            None => (MaterializationStrategy::Symlink, ConfigSource::Default),
        };
        let (mutation_durability, mutation_durability_source) = match raw.mutation_durability {
            Some(durability) => (durability, ConfigSource::File),
            None => (MutationDurability::Require, ConfigSource::Default),
        };

        Ok(ResolvedConfig {
            store,
            store_source,
            default_format,
            default_format_source,
            materialization,
            materialization_source,
            mutation_durability,
            mutation_durability_source,
        })
    }

    /// Kept for internal use by tests; delegates to `resolve_full`.
    #[cfg(test)]
    fn resolve(raw: RawConfig, store_override: Option<&Path>) -> Result<Self> {
        Ok(Self::resolve_full(raw, store_override)?.into())
    }

    /// Convenience constructor for tests: use an explicit store path with no
    /// config file read.
    #[cfg(test)]
    pub fn with_store(store: impl Into<PathBuf>) -> Self {
        Self {
            store: store.into(),
            default_format: None,
            materialization: MaterializationStrategy::Symlink,
            mutation_durability: MutationDurability::Require,
        }
    }
}

// ── Config file lookup ────────────────────────────────────────────────────────

/// Returns the platform-default path for the `config.toml` file.
///
/// - `$XDG_CONFIG_HOME/shelfbox/config.toml`, when `XDG_CONFIG_HOME` is an
///   absolute path
/// - Otherwise, the platform config directory plus `shelfbox/config.toml`
pub fn config_file_path() -> Option<PathBuf> {
    xdg_dir("XDG_CONFIG_HOME")
        .or_else(dirs::config_dir)
        .map(|d| d.join("shelfbox").join("config.toml"))
}

fn xdg_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).and_then(absolute_path)
}

fn absolute_path(value: OsString) -> Option<PathBuf> {
    let path = PathBuf::from(value);
    path.is_absolute().then_some(path)
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

    parse_raw_config(&path, &contents)
}

fn parse_raw_config(path: &Path, contents: &str) -> Result<RawConfig> {
    toml::from_str(contents).map_err(|e| AppError::toml_parse(path, e))
}

/// Sets a single key-value pair in the config file, preserving existing
/// content and comments.  Creates the file (and parent directories) if needed.
pub fn set_key(key: &str, value: &str) -> Result<()> {
    match key {
        "store" => {}
        "default_format" => match value {
            "table" | "plain" | "json" => {}
            other => {
                return Err(AppError::Internal(format!(
                    "invalid value '{other}' for default_format; valid values: table, plain, json"
                )));
            }
        },
        "materialization" => match value {
            "symlink" | "copy" => {}
            other => {
                return Err(AppError::Internal(format!(
                    "invalid value '{other}' for materialization; valid values: symlink, copy"
                )));
            }
        },
        "mutation_durability" => match value {
            "require" | "best-effort" => {}
            other => {
                return Err(AppError::Internal(format!(
                    "invalid value '{other}' for mutation_durability; valid values: require, best-effort"
                )));
            }
        },
        other => {
            return Err(AppError::Internal(format!(
                "unknown config key '{other}'; supported keys: store, default_format, materialization, mutation_durability"
            )));
        }
    }

    let path = config_file_path()
        .ok_or_else(|| AppError::Internal("could not determine config file path".into()))?;

    // Read the existing file or start with an empty document to preserve
    // any user-written comments and unknown keys.
    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(AppError::io(&path, e)),
    };

    let mut doc = contents.parse::<toml_edit::DocumentMut>().map_err(|e| {
        AppError::Internal(format!("config parse error in '{}': {e}", path.display()))
    })?;

    doc[key] = toml_edit::value(value);
    write_config_atomic(&path, &doc.to_string())
}

/// Writes `contents` to `path` atomically via a same-directory temp file and
/// `rename(2)`, so a crash mid-write cannot leave the config file truncated.
fn write_config_atomic(path: &Path, contents: &str) -> Result<()> {
    atomic_write::write(path, contents, ParentDirMode::Default)
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
            ..Default::default()
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
            ..Default::default()
        };
        let cfg = Config::resolve(raw, None).unwrap();
        assert_eq!(cfg.store, PathBuf::from("/from/config"));
    }

    #[test]
    fn falls_back_to_platform_default_when_both_absent() {
        let _g = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("SHELFBOX_STORE");
        let raw = RawConfig::default();
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
            ..Default::default()
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
            ..Default::default()
        };
        let override_path = PathBuf::from("/from/override");
        let cfg = Config::resolve(raw, Some(&override_path)).unwrap();
        std::env::remove_var("SHELFBOX_STORE");
        assert_eq!(cfg.store, override_path);
    }

    #[test]
    fn resolve_full_records_source_cli_flag() {
        let _g = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("SHELFBOX_STORE");
        let raw = RawConfig {
            store: Some(PathBuf::from("/from/config")),
            ..Default::default()
        };
        let override_path = PathBuf::from("/from/override");
        let r = Config::resolve_full(raw, Some(&override_path)).unwrap();
        assert_eq!(r.store, override_path);
        assert_eq!(r.store_source, ConfigSource::CliFlag);
        assert_eq!(r.default_format, None);
        assert_eq!(r.default_format_source, ConfigSource::Default);
        assert_eq!(r.materialization, MaterializationStrategy::Symlink);
        assert_eq!(r.materialization_source, ConfigSource::Default);
        assert_eq!(r.mutation_durability, MutationDurability::Require);
        assert_eq!(r.mutation_durability_source, ConfigSource::Default);
    }

    #[test]
    fn resolve_full_records_source_env() {
        let _g = ENV_MUTEX.lock().unwrap();
        std::env::set_var("SHELFBOX_STORE", "/from/env");
        let raw = RawConfig::default();
        let r = Config::resolve_full(raw, None).unwrap();
        std::env::remove_var("SHELFBOX_STORE");
        assert_eq!(r.store, PathBuf::from("/from/env"));
        assert_eq!(r.store_source, ConfigSource::Env("SHELFBOX_STORE"));
    }

    #[test]
    fn resolve_full_records_source_file() {
        let _g = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("SHELFBOX_STORE");
        let raw = RawConfig {
            store: Some(PathBuf::from("/from/config")),
            default_format: Some("json".to_string()),
            materialization: Some(MaterializationStrategy::Symlink),
            mutation_durability: Some(MutationDurability::Require),
        };
        let r = Config::resolve_full(raw, None).unwrap();
        assert_eq!(r.store_source, ConfigSource::File);
        assert_eq!(r.default_format.as_deref(), Some("json"));
        assert_eq!(r.default_format_source, ConfigSource::File);
        assert_eq!(r.materialization, MaterializationStrategy::Symlink);
        assert_eq!(r.materialization_source, ConfigSource::File);
        assert_eq!(r.mutation_durability, MutationDurability::Require);
        assert_eq!(r.mutation_durability_source, ConfigSource::File);
    }

    #[test]
    fn resolve_full_records_source_default() {
        let _g = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("SHELFBOX_STORE");
        let raw = RawConfig::default();
        let r = Config::resolve_full(raw, None).unwrap();
        assert_eq!(r.store_source, ConfigSource::Default);
        assert_eq!(r.default_format, None);
        assert_eq!(r.default_format_source, ConfigSource::Default);
        assert_eq!(r.materialization, MaterializationStrategy::Symlink);
        assert_eq!(r.materialization_source, ConfigSource::Default);
        assert_eq!(r.mutation_durability, MutationDurability::Require);
        assert_eq!(r.mutation_durability_source, ConfigSource::Default);
    }

    #[test]
    fn missing_materialization_defaults_to_symlink() {
        let _g = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("SHELFBOX_STORE");

        let config = Config::resolve(RawConfig::default(), None).unwrap();

        assert_eq!(config.materialization, MaterializationStrategy::Symlink);
    }

    #[test]
    fn missing_mutation_durability_defaults_to_require() {
        let _g = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("SHELFBOX_STORE");

        let config = Config::resolve(RawConfig::default(), None).unwrap();

        assert_eq!(config.mutation_durability, MutationDurability::Require);
    }

    #[test]
    fn mutation_durability_accepts_only_stable_values() {
        let best_effort = parse_raw_config(
            Path::new("config.toml"),
            "mutation_durability = \"best-effort\"\n",
        )
        .unwrap();
        assert_eq!(
            best_effort.mutation_durability,
            Some(MutationDurability::BestEffort)
        );
        assert!(matches!(
            parse_raw_config(
                Path::new("config.toml"),
                "mutation_durability = \"sometimes\"\n"
            ),
            Err(AppError::TomlParse { .. })
        ));
    }

    #[test]
    fn valid_materialization_values_parse_with_stable_names() {
        let symlink =
            parse_raw_config(Path::new("config.toml"), "materialization = \"symlink\"\n").unwrap();
        let copy =
            parse_raw_config(Path::new("config.toml"), "materialization = \"copy\"\n").unwrap();

        assert_eq!(
            symlink.materialization,
            Some(MaterializationStrategy::Symlink)
        );
        assert_eq!(copy.materialization, Some(MaterializationStrategy::Copy));
    }

    #[test]
    fn unknown_materialization_value_is_a_config_parse_error() {
        let error = parse_raw_config(Path::new("config.toml"), "materialization = \"hardlink\"\n")
            .unwrap_err();

        assert!(matches!(error, AppError::TomlParse { .. }));
    }

    #[test]
    fn copy_is_accepted_and_records_its_file_source() {
        let _g = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("SHELFBOX_STORE");
        let raw = RawConfig {
            materialization: Some(MaterializationStrategy::Copy),
            ..Default::default()
        };

        let resolved = Config::resolve_full(raw, None).unwrap();

        assert_eq!(resolved.materialization, MaterializationStrategy::Copy);
        assert_eq!(resolved.materialization_source, ConfigSource::File);
    }

    #[test]
    fn with_store_is_deterministic_and_uses_symlink_strategy() {
        let config = Config::with_store("/explicit-store");

        assert_eq!(config.store, PathBuf::from("/explicit-store"));
        assert_eq!(config.default_format, None);
        assert_eq!(config.materialization, MaterializationStrategy::Symlink);
    }

    #[test]
    fn write_config_atomic_replaces_file_and_cleans_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let tmp_path = dir.path().join(".shelfbox-config-write.tmp");

        write_config_atomic(&path, "store = \"/tmp/a\"\n").unwrap();
        write_config_atomic(&path, "default_format = \"json\"\n").unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "default_format = \"json\"\n"
        );
        assert!(!tmp_path.exists());
    }
}
