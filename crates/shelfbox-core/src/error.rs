use std::path::PathBuf;

use thiserror::Error;

use crate::domain::materialization::MaterializationStrategy;

/// Filesystem guarantees that may be unavailable on a platform or filesystem.
///
/// Callers must fail closed when a required capability is unavailable. In
/// particular, these capabilities must never be emulated with a
/// delete-then-create sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesystemCapability {
    NoFollowInspection,
    StableFileIdentity,
    LinkCount,
    AtomicReplaceRegularFile,
    AtomicReplaceSymlinkOrReparsePoint,
    DirectoryDurability,
}

impl std::fmt::Display for FilesystemCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::NoFollowInspection => "no-follow inspection",
            Self::StableFileIdentity => "stable file identity",
            Self::LinkCount => "link count",
            Self::AtomicReplaceRegularFile => "atomic regular-file replacement",
            Self::AtomicReplaceSymlinkOrReparsePoint => "atomic symlink/reparse-point replacement",
            Self::DirectoryDurability => "directory durability",
        };
        f.write_str(name)
    }
}

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

    /// The supplied path is a directory; use `add_directory` for namespace shelving.
    #[error(
        "'{path}' is a directory; use 'shelfbox item add {path}' to shelve all files inside it"
    )]
    PathIsDirectory { path: PathBuf },

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

    // ── item relink ───────────────────────────────────────────────────────────
    /// The item exists in the manifest but is not in `Detached` state.
    #[error(
        "'{path}' is not detached (current state: {actual_state})\n\
         hint: 'item relink' only applies to items in 'detached' state\n\
         hint: use 'item repair' to fix symlinks for attached items"
    )]
    RelinkNotDetached { path: PathBuf, actual_state: String },

    // ── item sync ─────────────────────────────────────────────────────────────
    /// Explicit synchronization cannot create a missing materialization,
    /// because that would silently choose a strategy. Repair it first.
    #[error("'{path}' is missing its materialization; run 'shelfbox item repair {path}' first")]
    SyncMaterializationMissing { path: PathBuf },

    /// Repository-to-store sync is deliberately an explicit destructive
    /// operation even though it uses an atomic replacement primitive.
    #[error("syncing repository content into the canonical store requires --yes")]
    SyncConfirmationRequired,

    /// The requested direction requires a currently attached isolated regular
    /// copy, not a managed symlink or another filesystem entry.
    #[error("'{path}' must be an attached isolated regular copy for this sync direction")]
    SyncRequiresRegularCopy { path: PathBuf },

    /// Lifecycle commands never choose which diverged Copy wins.  The user
    /// must select an explicit `item sync` direction first.
    #[error(
        "'{path}' diverges from its canonical store copy; run 'shelfbox item sync --from store' or '--from repo --yes' first"
    )]
    ContentDivergedRequiresSync { path: PathBuf },

    // ── Store format ─────────────────────────────────────────────────────────
    /// The manifest file uses an incompatible format version and cannot be
    /// loaded.  The store was written by a different version of shelfbox.
    #[error(
        "manifest at '{path}' has version {found}, expected {expected}\n\
         hint: run 'shelfbox store migrate-manifests' for legacy manifests"
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
        source: Box<std::io::Error>,
    },

    /// Failed to parse or serialize JSON (index / manifest).
    #[error("JSON error on '{path}': {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: Box<serde_json::Error>,
    },

    /// Failed to parse TOML config.
    #[error("config parse error in '{path}': {source}")]
    TomlParse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },

    /// A syntactically valid strategy is present in configuration, but its
    /// workflow has not passed the release safety gate yet.
    #[error(
        "materialization strategy '{strategy}' is not available in this build yet; \
         copy materialization will be enabled after its safety checks are complete"
    )]
    MaterializationStrategyUnavailable { strategy: MaterializationStrategy },

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
        source: Box<std::io::Error>,
    },

    // ── Durable recovery records ──────────────────────────────────────────
    #[error("malformed operation record at '{path}': {reason}")]
    OperationRecordMalformed { path: PathBuf, reason: String },

    #[error(
        "operation record at '{path}' uses unsupported schema version {found} (supported: {supported}); it was left untouched"
    )]
    OperationRecordUnsupportedVersion {
        path: PathBuf,
        found: u32,
        supported: u32,
    },

    #[error("recovery must be resolved before mutation (record {record_id}): {reason}")]
    RecoveryBlocked { record_id: String, reason: String },

    #[error("recovery artifact conflict for record {record_id} at '{path}': {reason}")]
    RecoveryArtifactConflict {
        record_id: String,
        path: PathBuf,
        reason: String,
    },

    /// A strict shelf mutation cannot start because the platform has no
    /// documented parent-directory durability primitive. This is deliberately
    /// raised at the operation boundary; low-level adapters retain
    /// `FilesystemCapabilityUnavailable`.
    #[error(
        "{operation} requires crash-safe directory durability, which is unavailable on {platform}"
    )]
    MutationDurabilityUnavailable {
        operation: String,
        platform: &'static str,
        capability: FilesystemCapability,
        #[source]
        source: Box<AppError>,
    },

    /// Recovery must not silently continue an operation that was started with
    /// a reduced durability contract after the user has returned to `require`.
    #[error(
        "recovery record {record_id} was created with best-effort durability; set mutation_durability to best-effort to resume or inspect recovery"
    )]
    MutationDurabilityRecoveryOptInRequired { record_id: String },

    // ── Platform capabilities ──────────────────────────────────────────────
    /// A required filesystem guarantee is unavailable on this platform or
    /// filesystem. The operation must stop without changing the destination.
    #[error("filesystem capability '{capability}' is unavailable on {platform}: {reason}")]
    FilesystemCapabilityUnavailable {
        capability: FilesystemCapability,
        platform: &'static str,
        reason: &'static str,
    },

    #[error("unsafe filesystem entry at '{path}': {reason}")]
    UnsafeFilesystemEntry { path: PathBuf, reason: &'static str },

    #[error("hardlinked file is not supported for this operation: {path}")]
    HardlinkedFile { path: PathBuf },

    #[error("filesystem entry changed during transfer: {path}")]
    FilesystemEntryChanged { path: PathBuf },

    // ── Internal / unexpected ─────────────────────────────────────────────
    /// An invariant was violated that should never occur in correct usage.
    /// Wraps a human-readable description for debugging.
    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    /// Stable machine-facing classification for errors that require a user
    /// policy decision. Other errors retain their existing detailed variants.
    pub fn classification(&self) -> Option<&'static str> {
        match self {
            Self::MutationDurabilityUnavailable { .. } => Some("mutation_durability_unavailable"),
            _ => None,
        }
    }

    /// Convenience constructor for `AppError::Io`.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source: Box::new(source),
        }
    }

    /// Convenience constructor for `AppError::Json`.
    pub fn json(path: impl Into<PathBuf>, source: serde_json::Error) -> Self {
        Self::Json {
            path: path.into(),
            source: Box::new(source),
        }
    }

    /// Convenience constructor for `AppError::TomlParse`.
    pub fn toml_parse(path: impl Into<PathBuf>, source: toml::de::Error) -> Self {
        Self::TomlParse {
            path: path.into(),
            source: Box::new(source),
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
