/// Output format selection for list/status commands.
///
/// `Detail` is a table format with the full field set (verbosity = verbose).
/// Future: split into `--format <fmt>` + `--verbose` flag.
#[derive(Clone, Debug, Default, clap::ValueEnum)]
pub enum OutputFormat {
    /// Aligned columns (default).
    #[default]
    Table,
    /// One entry per line, minimal whitespace.
    Plain,
    /// Machine-readable JSON.
    Json,
    /// Verbose table with extended fields.
    Detail,
}
