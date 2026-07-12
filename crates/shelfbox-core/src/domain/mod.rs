//! Persistent domain concepts shared across storage and operations.
//!
//! Domain modules own data shapes and small invariants. Store modules continue
//! to own filesystem I/O and serialized file placement.

#[allow(dead_code)] // D5 contract; consumed by copy mutation journal/materializer phases.
pub(crate) mod copy_safety;
pub mod ids;
pub mod index;
pub mod manifest;
pub mod materialization;
pub(crate) mod operation_record;
pub mod ownership;
pub mod path;
pub mod recovery_fingerprint;
