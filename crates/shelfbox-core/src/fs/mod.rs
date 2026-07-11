#[allow(dead_code)] // D6 contract; concrete adapters arrive in Phase 3.
pub(crate) mod canonical_transfer;
pub(crate) mod file_identity;
pub(crate) mod lock;
#[allow(dead_code)] // D6 contract; consumed by operation migration in Phase 3.
pub(crate) mod materializer;
pub(crate) mod paths;
pub(crate) mod permissions;
#[allow(dead_code)] // D1 spike; consumed by secure transfer in Phase 2.
pub(crate) mod platform;
pub(crate) mod secure_transfer;
pub mod symlink;
