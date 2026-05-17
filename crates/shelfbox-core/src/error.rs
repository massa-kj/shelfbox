use std::path::PathBuf;

use thiserror::Error;

/// Top-level error type for shelfbox-core.
///
/// Variants are kept fine-grained so callers (CLI, GUI, tests) can match
/// on specific conditions without parsing error strings.
#[derive(Debug, Error)]
pub enum AppError {
    // ── Context / environment ──────────────────────────────────────────────
    /// The current directory is not inside a Git repository.
    #[error("not inside a git repository")]
    NotAGitRepo,

    /// Failed to determine the Git repository root.
    #[error("failed to locate git repository root: {0}")]
    GitRootDetection(String),

    // ── Path validation ────────────────────────────────────────────────────
    /// The supplied path lies outside the repository root.
    #[error("path is outside the git repository: {path}")]
    PathOutsideRepo { path: PathBuf },

    /// The supplied path is inside the `.git/` directory.
    #[error("path is inside .git/ and cannot be shelved: {path}")]
    PathInsideGitDir { path: PathBuf },

    /// The supplied path is already tracked by Git.
    #[error("'{path}' is tracked by git; shelving tracked files is not allowed by default")]
    PathIsTracked { path: PathBuf },

    /// The supplied path is already a symlink.
    #[error("'{path}' is already a symlink; shelving symlinks is not supported in this version")]
    PathIsSymlink { path: PathBuf },

    /// The supplied path is already managed by shelfbox.
    #[error("'{path}' is already managed by shelfbox")]
    AlreadyManaged { path: PathBuf },

    // ── Store conflicts ────────────────────────────────────────────────────
    /// A file already exists in the store at the computed target path.
    #[error("store conflict: '{store_path}' already exists")]
    StoreConflict { store_path: PathBuf },

    // ── repair validation ──────────────────────────────────────────────────
    /// A regular (non-symlink) file exists at the repo path; overwriting it
    /// would cause data loss, so `repair` refuses to proceed.
    #[error("'{path}' is a regular file; refusing to overwrite (use 'shelfbox restore' first)")]
    PathIsRegularFile { path: PathBuf },

    // ── restore validation ─────────────────────────────────────────────────    /// The restore destination is occupied by a non-symlink entry (regular
    /// file or directory). Overwriting it would cause data loss.
    #[error(
        "restore destination already exists as a regular file or directory: {path}\n\
         hint: move or rename the existing file first, then re-run restore"
    )]
    RestoreDestinationExists { path: PathBuf },
    /// The path at the repo side is not a shelfbox managed symlink.
    #[error("'{path}' is not a shelfbox managed symlink")]
    NotManagedLink { path: PathBuf },

    /// The store-side item is missing (dangling link).
    #[error("store item not found for '{path}': expected at '{store_path}'")]
    StoreMissing { path: PathBuf, store_path: PathBuf },

    // ── I/O and data ──────────────────────────────────────────────────────
    /// A generic I/O error, annotated with the path that caused it.
    #[error("I/O error on '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Failed to parse or serialize JSON (index / manifest).
    #[error("JSON error on '{path}': {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// Failed to parse TOML config.
    #[error("config parse error in '{path}': {source}")]
    TomlParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    // ── Git subprocess ─────────────────────────────────────────────────────
    /// A `git` subprocess failed to spawn or returned a non-zero exit code.
    #[error("git command failed: {message}")]
    GitCommand { message: String },

    // ── Internal / unexpected ─────────────────────────────────────────────
    /// An invariant was violated that should never occur in correct usage.
    /// Wraps a human-readable description for debugging.
    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    /// Convenience constructor for `AppError::Io`.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// Convenience constructor for `AppError::Json`.
    pub fn json(path: impl Into<PathBuf>, source: serde_json::Error) -> Self {
        Self::Json {
            path: path.into(),
            source,
        }
    }

    /// Convenience constructor for `AppError::TomlParse`.
    pub fn toml_parse(path: impl Into<PathBuf>, source: toml::de::Error) -> Self {
        Self::TomlParse {
            path: path.into(),
            source,
        }
    }

    /// Convenience constructor for `AppError::GitCommand`.
    pub fn git_command(message: impl Into<String>) -> Self {
        Self::GitCommand {
            message: message.into(),
        }
    }
}

/// Alias used throughout `shelfbox-core` to keep signatures concise.
pub type Result<T, E = AppError> = std::result::Result<T, E>;
