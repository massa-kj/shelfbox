//! Core library for shelfbox.
//!
//! The preferred operational boundary is [`api`]. Legacy modules such as
//! [`context`], [`ops`], [`store`], [`git`], [`ignore`], and [`link`] remain
//! public for compatibility while the refactor migrates callers behind the API
//! facade. Lower-level filesystem and storage adapters are crate-private.

pub mod api;
pub mod config;
pub mod context;
pub mod domain;
pub mod error;
pub(crate) mod fs;
pub mod git;
pub mod ignore;
pub mod link;
pub mod ops;
pub mod plan;
pub(crate) mod policy;
pub(crate) mod storage;
pub mod store;

// Re-export the most commonly used items so downstream crates only need
// `use shelfbox_core::prelude::*` for the essentials.
pub mod prelude {
    pub use crate::error::{AppError, Result};
}
