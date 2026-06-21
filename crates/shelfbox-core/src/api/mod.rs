//! Public operational facade for CLI and downstream callers.
//!
//! The first phase keeps these modules thin and delegates to existing
//! implementation modules. Later phases can move behavior behind this facade
//! without forcing CLI call sites to keep importing internals directly.

pub mod config;
pub mod item;
pub mod repo;
pub mod store;
