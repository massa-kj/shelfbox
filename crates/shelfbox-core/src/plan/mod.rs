//! Operation plans used to keep dry-run and execution paths aligned.
//!
//! Plan builders validate and describe the intended work without mutating the
//! store or working tree. Operation modules execute those plans and return the
//! same shape to callers for presentation.

pub mod item_move;
pub mod item_restore;
pub mod manifest_migration;
pub mod store_gc;
pub mod store_rebuild_index;
