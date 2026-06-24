/// Output format selection for list/status commands.
#[derive(Clone, Debug, Default, clap::ValueEnum)]
pub enum OutputFormat {
    /// Aligned columns (default).
    #[default]
    Table,
    /// One entry per line, minimal whitespace.
    Plain,
    /// Machine-readable JSON.
    Json,
}

impl OutputFormat {
    /// Resolve the effective format from an explicit CLI argument and the
    /// `default_format` config value.  Falls back to [`OutputFormat::Table`]
    /// when both are absent or unrecognised.
    pub fn resolve(explicit: Option<Self>, default_format: &Option<String>) -> Self {
        explicit.unwrap_or_else(|| {
            default_format
                .as_deref()
                .and_then(Self::from_config_str)
                .unwrap_or_default()
        })
    }

    fn from_config_str(s: &str) -> Option<Self> {
        match s {
            "table" => Some(Self::Table),
            "plain" => Some(Self::Plain),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}
