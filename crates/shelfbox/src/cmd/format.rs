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
    /// Verbose key-value pairs (not yet implemented).
    Detail,
}
