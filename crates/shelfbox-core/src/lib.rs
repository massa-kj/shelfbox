pub mod api;
pub mod config;
pub mod context;
pub mod error;
pub mod git;
pub mod ignore;
pub mod link;
pub mod ops;
pub mod plan;
pub mod store;

// Re-export the most commonly used items so downstream crates only need
// `use shelfbox_core::prelude::*` for the essentials.
pub mod prelude {
    pub use crate::error::{AppError, Result};
}
