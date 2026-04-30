use crate::{context::RepoContext, store::manifest::Item};

/// Returns all items currently shelved in this repository.
///
/// The returned slice is borrowed from `ctx`; it reflects the in-memory
/// manifest at the time of the call.
pub fn list(ctx: &RepoContext) -> &[Item] {
    &ctx.manifest.items
}
