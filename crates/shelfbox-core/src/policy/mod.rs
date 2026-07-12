//! Safety and eligibility rules for shelfbox operations.
//!
//! Policy modules take already-collected facts and decide what is allowed.
//! Filesystem, Git, and storage I/O should stay in their owning modules.

pub(crate) mod gc_policy;
pub(crate) mod item_validation;
pub(crate) mod materialization_policy;
pub(crate) mod migration_policy;
pub(crate) mod path_escape_policy;
pub(crate) mod reclaim_policy;
pub(crate) mod recovery_policy;
pub(crate) mod repair_policy;
