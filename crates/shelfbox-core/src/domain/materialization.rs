//! Strategy-neutral materialization vocabulary.
//!
//! The strategy is domain data. Filesystem adapters decide how to execute it,
//! while operations use it only to select an approved typed action.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterializationStrategy {
    Symlink,
    Copy,
}
