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

    // ── move validation ─────────────────────────────────────────────────────
    /// The destination path for `item move` is already occupied.
    #[error("move destination already exists: {path}")]
    MoveDestinationExists { path: PathBuf },

    /// The symlink at the move source does not point to the expected store path.
    /// Running `item repair` first will re-establish the correct link.
    #[error(
        "symlink at '{path}' does not point to expected store location\n\
         hint: run 'shelfbox item repair' on this path first"
    )]
    MoveSourceSymlinkMismatch { path: PathBuf },

    /// Moving directory items is not supported in this version.
    #[error("moving directory items is not supported in this version")]
    MoveDirectoryUnsupported,

    // ── repair validation ──────────────────────────────────────────────────
    /// A regular (non-symlink) file exists at the repo path; overwriting it
    /// would cause data loss, so `repair` refuses to proceed.
    #[error("'{path}' is a regular file; refusing to overwrite (use 'shelfbox restore' first)")]
    PathIsRegularFile { path: PathBuf },

    /// A symlink exists at the repo path but points to an unexpected target.
    /// Overwriting it silently could mask a wrong machine, stale store, or
    /// copied-repo situation.  Use `repair --force` to override explicitly.
    #[error(
        "symlink target mismatch at '{path}': points to '{actual_target}', expected '{expected_target}'\n\
         hint: run 'shelfbox item repair --force' if this is intentional"
    )]
    RepairSymlinkTargetMismatch {
        path: PathBuf,
        actual_target: PathBuf,
        expected_target: PathBuf,
    },

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

    // ── directory namespaces ──────────────────────────────────────────────────
    /// A nested Git repository was detected inside the directory being shelved.
    /// Directory shelving does not cross git repository boundaries.
    #[error(
        "nested git repository found at '{path}'\n\
         hint: shelve individual files instead, or remove the nested repository first"
    )]
    NestedGitRepo { path: PathBuf },

    /// The specified directory namespace is not registered in the manifest.
    #[error(
        "no namespace registered for '{path}'\n\
         hint: run 'shelfbox item add {path}' to shelve the directory first"
    )]
    NamespaceNotFound { path: String },

    // ── repo adopt ────────────────────────────────────────────────────────────
    /// Cannot adopt items from the current repository.
    #[error("cannot adopt from self (repo id: '{repo_id}')")]
    AdoptFromSelf { repo_id: String },

    /// The specified source repository is not registered in the store index.
    #[error(
        "no store entry found for repo id '{repo_id}'\n\
         hint: run 'shelfbox repo list' to see known repositories"
    )]
    AdoptSourceNotFound { repo_id: String },

    // ── Store format ─────────────────────────────────────────────────────────
    /// The manifest file uses an incompatible format version and cannot be
    /// loaded.  The store was written by a different version of shelfbox.
    #[error(
        "manifest at '{path}' has version {found}, expected {expected}\n\
         hint: re-shelve your items to migrate to the new format"
    )]
    ManifestVersionMismatch {
        path: PathBuf,
        found: u32,
        expected: u32,
    },

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

    // ── Store lock ────────────────────────────────────────────────────────
    /// Another `shelfbox` process is holding the store lock.
    #[error(
        "store is locked by another process: {lock_path}\n\
         hint: wait for the other shelfbox invocation to finish, then retry"
    )]
    StoreLocked {
        lock_path: PathBuf,
        #[source]
        source: std::io::Error,
    },

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
