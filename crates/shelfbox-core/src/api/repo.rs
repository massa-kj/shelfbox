use std::path::Path;

pub use crate::{
    context::{
        CurrentGitContext, ExplicitReclaimContext, ReadOnlyRepoContext, RepoContext, StoreAccess,
        StoreContext,
    },
    ops::{
        detect_transitions::TransitionReport,
        integrity::IntegrityReport,
        reclaim::{CandidateState, ReclaimCandidate, ReclaimOutcome},
    },
    plan::repo_repair::{RepairRepoReport, RepoRepairPlan, RepoRepairSymlinkAction},
    store::{
        index::{Index, RepoEntry},
        manifest::{Manifest, OwnershipState},
    },
};

use crate::{
    config::Config,
    context,
    error::Result,
    ignore::GitInfoExclude,
    link::DefaultLinkStrategy,
    ops::{detect_transitions, integrity, reclaim, repair},
    store::{index, manifest},
};

pub fn load_config(store_override: Option<&Path>) -> Result<Config> {
    Config::load(store_override)
}

pub fn load_index(store_root: &Path) -> Result<Index> {
    index::load(store_root)
}

pub fn load_manifest(repo_store: &Path) -> Result<Manifest> {
    manifest::load(repo_store)
}

pub fn current_git_context(cwd: &Path) -> Result<CurrentGitContext> {
    context::current_git_context(cwd)
}

pub fn resolve_existing_repo(current: &CurrentGitContext, index: &Index) -> Option<String> {
    context::resolve_existing_repo(current, index)
}

pub fn build_store_context(
    store_override: Option<&Path>,
    access: StoreAccess,
) -> Result<StoreContext> {
    context::build_store_context(store_override, access)
}

pub fn build_create_or_load(cwd: &Path, store_override: Option<&Path>) -> Result<RepoContext> {
    context::build_create_or_load(cwd, store_override)
}

pub fn build_read_only(cwd: &Path, store_override: Option<&Path>) -> Result<ReadOnlyRepoContext> {
    context::build_read_only(cwd, store_override)
}

pub fn build_explicit_reclaim(
    cwd: &Path,
    store_override: Option<&Path>,
    target_repo_id: impl Into<String>,
) -> Result<ExplicitReclaimContext> {
    context::build_explicit_reclaim(cwd, store_override, target_repo_id)
}

pub fn integrity_check(ctx: &RepoContext) -> Result<IntegrityReport> {
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    integrity::check(ctx, &link, &ignore)
}

pub fn scan_transitions(ctx: &RepoContext, config: &Config) -> Result<TransitionReport> {
    detect_transitions::scan(ctx, config)
}

pub fn repair_repo(ctx: &mut RepoContext, dry_run: bool, force: bool) -> Result<RepairRepoReport> {
    let link = DefaultLinkStrategy;
    repair::repair_repo(ctx, &link, dry_run, force)
}

pub fn check_reclaim_precondition(current_manifest: Option<&Manifest>) -> Result<()> {
    reclaim::check_reclaim_precondition(current_manifest)
}

pub fn build_reclaim_candidates(
    store_root: &Path,
    current_repo_root: &Path,
    current_remote_hint: Option<&str>,
    index: &Index,
) -> Result<Vec<ReclaimCandidate>> {
    reclaim::build_candidates(store_root, current_repo_root, current_remote_hint, index)
}

pub fn execute_reclaim(ctx: &ExplicitReclaimContext) -> Result<ReclaimOutcome> {
    reclaim::execute_reclaim(&ctx.config.store, &ctx.current, &ctx.target_repo_id)
}
