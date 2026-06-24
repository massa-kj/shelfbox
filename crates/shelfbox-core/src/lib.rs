//! Core library for shelfbox.
//!
//! The public operational boundary is [`api`]. Lower-level context,
//! operation, storage, Git, ignore, link, filesystem, and policy modules are
//! crate-private implementation details.

pub mod api;
pub mod config;
pub(crate) mod context;
pub mod domain;
pub mod error;
pub(crate) mod fs;
pub(crate) mod git;
pub(crate) mod ignore;
pub(crate) mod link;
pub(crate) mod ops;
pub mod plan;
pub(crate) mod policy;
pub(crate) mod storage;
pub(crate) mod store;

// Re-export the most commonly used items so downstream crates only need
// `use shelfbox_core::prelude::*` for the essentials.
pub mod prelude {
    pub use crate::error::{AppError, Result};
}

#[cfg(test)]
extern crate self as shelfbox_core;

#[cfg(test)]
#[path = "integration_tests/common/mod.rs"]
mod integration_test_common;

#[cfg(test)]
#[path = "integration_tests/chaos_integration.rs"]
mod chaos_integration;

#[cfg(test)]
#[path = "integration_tests/context_modes.rs"]
mod context_modes;

#[cfg(test)]
#[path = "integration_tests/ops_integration.rs"]
mod ops_integration;

#[cfg(test)]
#[path = "integration_tests/recovery_integration.rs"]
mod recovery_integration;

#[cfg(test)]
#[path = "integration_tests/scenario_integration.rs"]
mod scenario_integration;
