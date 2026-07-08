//! Persistent domain concepts shared across storage and operations.
//!
//! Domain modules own data shapes and small invariants. Store modules continue
//! to own filesystem I/O and serialized file placement.

pub mod ids;
pub mod index;
pub mod manifest;
pub mod ownership;
pub mod path;
pub mod recovery_fingerprint;
