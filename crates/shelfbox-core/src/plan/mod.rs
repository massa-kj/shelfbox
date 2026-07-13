//! Operation plans used to keep dry-run and execution paths aligned.
//!
//! Plan builders validate and describe the intended work without mutating the
//! store or working tree. Operation modules execute those plans and return the
//! same shape to callers for presentation.

pub mod item_add;
pub mod item_move;
pub mod item_relink;
pub mod item_repair;
pub mod item_restore;
pub mod item_sync;
pub mod manifest_migration;
pub mod repo_reclaim;
pub mod repo_repair;
pub mod store_gc;
pub mod store_rebuild_index;
