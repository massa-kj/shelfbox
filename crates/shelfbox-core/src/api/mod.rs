//! Public operational facade for CLI and downstream callers.
//!
//! Callers should enter core behavior through this module instead of importing
//! lower-level implementation modules. The facade owns stable operation
//! groupings while context construction, storage I/O, Git integration,
//! filesystem adapters, and policy decisions remain crate-private details.

pub mod config;
pub mod item;
pub mod repo;
pub mod store;
