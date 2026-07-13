//! Typed plan and report vocabulary for explicit item content synchronization.
//!
//! A direction is deliberately one enum value rather than a pair of booleans:
//! this makes an omitted or conflicting direction unrepresentable to core
//! callers.

use std::path::PathBuf;

/// The source of truth selected for an explicit content synchronization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDirection {
    /// Replace the repository regular copy with the canonical store content.
    FromStore,
    /// Replace canonical store content with the repository regular copy.
    FromRepo,
}

/// Policy-approved result selected during planning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemSyncAction {
    /// Both isolated regular files already have equal content.
    AlreadySynchronized,
    /// A managed symlink already reads canonical content, so no write exists.
    ManagedSymlinkNoOp,
    /// Atomically replace the repository copy using canonical store content.
    ReplaceRepoFromStore,
    /// Atomically replace the canonical store file using repository content.
    ReplaceStoreFromRepo,
}

/// User-visible outcome after executing (or planning) synchronization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncOutcome {
    AlreadySynchronized,
    ManagedSymlinkNoOp,
    SynchronizedFromStore,
    SynchronizedFromRepo,
    WouldSynchronizeFromStore,
    WouldSynchronizeFromRepo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemSyncPlan {
    pub path: String,
    pub abs_path: PathBuf,
    pub store_path: PathBuf,
    pub direction: SyncDirection,
    pub action: ItemSyncAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemSyncReport {
    pub plan: ItemSyncPlan,
    pub outcome: SyncOutcome,
    pub dry_run: bool,
}

/// Options for the item sync API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemSyncRequest {
    pub direction: SyncDirection,
    pub dry_run: bool,
    /// Required only when a `FromRepo` plan would write canonical content.
    pub confirmed: bool,
}
