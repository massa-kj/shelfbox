use std::path::{Path, PathBuf};

use ulid::Ulid;

use super::path::{normalize_repo_relative, repo_relative_path, repo_relative_string};
use crate::{
    context::{self, RepoContext},
    domain::{
        materialization::MaterializationStrategy,
        operation_record::{
            OperationKind, OperationPhase, OperationPreState, OperationRecord,
            RecoveryAbsolutePath, RecoveryRecord, RecoveryRecordKind,
            OPERATION_RECORD_SCHEMA_VERSION,
        },
        path::{RepoRelativePath, StoreRelativePath},
        recovery_fingerprint::RecoveryFingerprint,
    },
    error::{AppError, Result},
    failpoint::{self, Failpoint},
    fs::{
        canonical_transfer::{
            CanonicalEntryKind, CanonicalInspectionPurpose, CanonicalTransfer,
            CanonicalTransferAction, CanonicalTransferInspectionRequest, DefaultCanonicalTransfer,
        },
        materializer::{
            DefaultMaterializer, InspectionPurpose, MaterializationAction,
            MaterializationInspectionRequest, MaterializationLocation, Materializer,
            MutationJournal, RepoEntryKind,
        },
        mutation_journal::AddMutationJournal,
    },
    git,
    ignore::IgnoreBackend,
    link::LinkStrategy,
    plan::item_add::{ItemAddPlan, ItemAddReport},
    policy::item_validation::{self, DirectoryCandidateDecision},
    storage::operation_record_store,
    store::manifest::{self, Item, OwnershipState},
};

fn update_identity_hints(ctx: &mut RepoContext) {
    if let Some(name) = ctx.repo_root.file_name().and_then(|n| n.to_str()) {
        ctx.manifest.add_repo_name_hint(name);
    }

    if let Ok(Some(remote_url)) = git::remote_url(&ctx.repo_root) {
        if let Some(remote_hint) = git::normalize_remote_hint(&remote_url) {
            ctx.manifest.add_remote_hint(&remote_hint);
        }
    }
}

pub(crate) fn add_report(
    ctx: &mut RepoContext,
    abs_path: &Path,
    dry_run: bool,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<ItemAddReport> {
    let plan = build_add_plan(ctx, abs_path)?;

    if !dry_run {
        execute_add_plan(ctx, &plan, link, ignore)?;
    }

    Ok(ItemAddReport { plan, dry_run })
}

fn build_add_plan(ctx: &RepoContext, abs_path: &Path) -> Result<ItemAddPlan> {
    // ── Path validation ──────────────────────────────────────────────────────
    // Must be within the repository root.
    let rel_path = repo_relative_path(&ctx.repo_root, abs_path)?;
    let rel_str = repo_relative_string(&ctx.repo_root, abs_path)?;

    item_validation::validate_add_location(&rel_path, abs_path)?;

    // Read symlink metadata so we can distinguish symlinks from regular entries
    // without following the link (also validates the path exists).
    let meta = abs_path
        .symlink_metadata()
        .map_err(|e| AppError::io(abs_path, e))?;
    let kind = item_validation::add_entry_kind_from_meta(&meta);

    item_validation::validate_add_entry_kind(abs_path, kind)?;

    // Must not be tracked by Git.
    item_validation::validate_add_git_state(abs_path, git::is_tracked(&ctx.repo_root, abs_path)?)?;

    // Must not already be managed by shelfbox.
    item_validation::validate_add_manifest_state(abs_path, ctx.manifest.contains(&rel_str))?;

    // Store destination must not already be occupied.
    let store_path = ctx.store_path_for(&rel_str);
    item_validation::validate_add_store_destination(&store_path, store_path.exists())?;

    // Store-relative path (relative to repo_store): "items/<rel>".
    let store_path_rel = item_validation::store_item_path_for_repo_path(&rel_str);
    let repo_path: RepoRelativePath =
        rel_str
            .parse()
            .map_err(|_| AppError::UnsafeFilesystemEntry {
                path: abs_path.to_path_buf(),
                reason: "add path is not normalized",
            })?;
    let store_path_global = store_relative_path(&ctx.config.store, &store_path)?;
    validate_add_canonical_planning(ctx, abs_path, &repo_path, &store_path_global)?;

    Ok(ItemAddPlan {
        path: rel_str,
        abs_path: abs_path.to_path_buf(),
        store_path,
        store_path_relative: store_path_rel,
    })
}

fn validate_add_canonical_planning(
    ctx: &RepoContext,
    abs_path: &Path,
    repo_path: &RepoRelativePath,
    store_path: &StoreRelativePath,
) -> Result<()> {
    let transfer = DefaultCanonicalTransfer::new(ctx.repo_root.clone(), ctx.config.store.clone());
    let action = CanonicalTransferAction::ReplaceFromRepo {
        source: repo_path.clone(),
        destination: store_path.clone(),
        expected_source: crate::fs::canonical_transfer::ExpectedCanonicalEntry::unchecked(
            CanonicalEntryKind::RegularFile,
        ),
        expected_destination: crate::fs::canonical_transfer::ExpectedCanonicalEntry::unchecked(
            CanonicalEntryKind::Missing,
        ),
    };
    let facts = transfer.inspect(CanonicalTransferInspectionRequest {
        action,
        purpose: CanonicalInspectionPurpose::Planning,
    })?;
    if facts.source_kind != CanonicalEntryKind::RegularFile {
        return Err(AppError::UnsafeFilesystemEntry {
            path: abs_path.to_path_buf(),
            reason: "add source is not an isolated regular file",
        });
    }
    if !facts.source_hardlink_free {
        return Err(AppError::HardlinkedFile {
            path: abs_path.to_path_buf(),
        });
    }
    if facts.destination_kind != CanonicalEntryKind::Missing || !facts.destination_hardlink_free {
        return Err(AppError::StoreConflict {
            store_path: ctx.config.store.join(store_path.as_str()),
        });
    }
    Ok(())
}

fn execute_add_plan(
    ctx: &mut RepoContext,
    plan: &ItemAddPlan,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<()> {
    let repo_path = RepoRelativePath::new(plan.path.clone()).ok_or_else(|| {
        AppError::UnsafeFilesystemEntry {
            path: plan.abs_path.clone(),
            reason: "add plan contains an unsafe repository path",
        }
    })?;
    let store_path = store_relative_path(&ctx.config.store, &plan.store_path)?;
    let source_fingerprint = RecoveryFingerprint::from_file(&plan.abs_path)?;
    let exclude_was_present = ignore.has_entry(&ctx.repo_root, &plan.path)?;
    let strategy = ctx.config.materialization;
    let store_root = ctx.config.store.clone();
    let repo_root = ctx.repo_root.clone();

    // The full operation record is durable before the first mutation. It
    // captures content, ownership, and path facts without copying plaintext.
    let mut record = RecoveryRecord {
        schema_version: OPERATION_RECORD_SCHEMA_VERSION,
        record_id: Ulid::new().to_string(),
        created_at: context::now_iso8601(),
        record: RecoveryRecordKind::Operation(OperationRecord {
            operation: OperationKind::Add,
            phase: OperationPhase::RecordCreated,
            repo_id: ctx.repo_id.clone(),
            repo_root: RecoveryAbsolutePath::new(&ctx.repo_root).ok_or_else(|| {
                AppError::UnsafeFilesystemEntry {
                    path: ctx.repo_root.clone(),
                    reason: "repository root is not a safe absolute path",
                }
            })?,
            strategy,
            direction: None,
            pre_state: OperationPreState {
                repo_path: Some(repo_path.clone()),
                store_path: Some(store_path.clone()),
                repo_fingerprint: Some(source_fingerprint),
                store_fingerprint: None,
                manifest_contains_item: Some(false),
                exclude_owned: Some(exclude_was_present),
            },
            artifact_record_ids: Vec::new(),
            backup: None,
        }),
    };
    operation_record_store::create(&ctx.config.store, &record)?;

    let mut journal = AddMutationJournal::new(
        &store_root,
        &repo_root,
        ignore,
        &mut record,
        plan.abs_path.clone(),
        plan.store_path.clone(),
    );

    // An exclude is established and verified before the repo entry is moved
    // or a regular copy can be materialized. Pre-existing user entries are
    // preserved: recovery removes only excludes this operation added.
    ignore.add_entries(&ctx.repo_root, &[&plan.path])?;
    failpoint::after(Failpoint::PersistentMutation(
        crate::domain::copy_safety::PersistentMutation::RepoTempExclude,
    ))?;
    if !ignore.has_entry(&ctx.repo_root, &plan.path)? {
        return Err(AppError::Internal(
            "managed add exclude was not persisted".into(),
        ));
    }
    journal.advance(OperationPhase::ExcludeWritten)?;
    journal.ensure_store_destination_parent()?;

    let transfer_action = CanonicalTransferAction::ReplaceFromRepo {
        source: repo_path.clone(),
        destination: store_path.clone(),
        expected_source: canonical_source_expectation(
            &ctx.repo_root,
            &ctx.config.store,
            &repo_path,
            &store_path,
        )?,
        expected_destination: canonical_destination_expectation(
            &ctx.repo_root,
            &ctx.config.store,
            &repo_path,
            &store_path,
        )?,
    };
    let mut transfer = DefaultCanonicalTransfer::new(repo_root.clone(), store_root.clone());
    let prepared_transfer = transfer.prepare(transfer_action.clone(), &mut journal)?;
    let transfer_facts = transfer.inspect(CanonicalTransferInspectionRequest {
        action: transfer_action.clone(),
        purpose: CanonicalInspectionPurpose::PreCommit,
    })?;
    validate_add_transfer_preconditions(ctx, plan, ignore, &transfer_facts)?;
    let transfer_permit = journal.issue_commit_permit(
        transfer_facts.write_precondition_guard(prepared_transfer.commit_context()),
    )?;
    transfer.commit(prepared_transfer, transfer_permit)?;
    journal.advance(OperationPhase::StoreTransferred)?;

    let location = MaterializationLocation::new(repo_path.clone(), store_path.clone());
    let materialization_action = MaterializationAction::Create {
        location: location.clone(),
        strategy,
    };
    let mut materializer =
        DefaultMaterializer::with_link_strategy(repo_root.clone(), store_root.clone(), link);
    let prepared_materialization = materializer.prepare(materialization_action, &mut journal)?;
    let materialization_facts = materializer.inspect(MaterializationInspectionRequest {
        location: location.clone(),
        purpose: InspectionPurpose::PreCommit,
    })?;
    validate_add_materialization_preconditions(ctx, plan, ignore, &materialization_facts)?;
    let materialization_permit = journal.issue_commit_permit(
        materialization_facts.write_precondition_guard(prepared_materialization.commit_context()),
    )?;
    materializer.commit(prepared_materialization, materialization_permit)?;
    journal.advance(OperationPhase::RepoMaterialized)?;

    // Record the item in the manifest only after both canonical movement and
    // materialization have completed. The durable operation record remains
    // until postconditions have been independently checked.
    let now = context::now_iso8601();
    update_identity_hints(ctx);
    let item = Item {
        item_id: Ulid::new().to_string(),
        origin_repo_id: ctx.repo_id.clone(),
        path: plan.path.clone(),
        store_path: plan.store_path_relative.clone(),
        ownership_state: OwnershipState::Attached,
        created_at: now.clone(),
        updated_at: now,
    };
    ctx.manifest.add(item);
    manifest::save(&ctx.repo_store, &ctx.manifest)?;
    journal.advance(OperationPhase::ManifestSaved)?;

    let post = materializer.inspect(MaterializationInspectionRequest {
        location,
        purpose: InspectionPurpose::PostCommit,
    })?;
    validate_add_postconditions(ctx, plan, ignore, strategy, &post)?;
    journal.advance(OperationPhase::PostCommitValidated)?;
    failpoint::after(Failpoint::PersistentMutation(
        crate::domain::copy_safety::PersistentMutation::PostMaterializationValidationRecord,
    ))?;
    journal.cleanup_all()?;
    operation_record_store::remove(&ctx.config.store, &record.record_id)?;

    Ok(())
}

fn store_relative_path(store_root: &Path, path: &Path) -> Result<StoreRelativePath> {
    let relative = path
        .strip_prefix(store_root)
        .map_err(|_| AppError::UnsafeFilesystemEntry {
            path: path.to_path_buf(),
            reason: "store destination escapes the configured store root",
        })?;
    StoreRelativePath::new(relative.to_string_lossy().replace('\\', "/")).ok_or_else(|| {
        AppError::UnsafeFilesystemEntry {
            path: path.to_path_buf(),
            reason: "store destination is not normalized",
        }
    })
}

fn canonical_source_expectation(
    repo_root: &Path,
    store_root: &Path,
    repo_path: &RepoRelativePath,
    store_path: &StoreRelativePath,
) -> Result<crate::fs::canonical_transfer::ExpectedCanonicalEntry> {
    let transfer = DefaultCanonicalTransfer::new(repo_root.to_path_buf(), store_root.to_path_buf());
    let action = CanonicalTransferAction::ReplaceFromRepo {
        source: repo_path.clone(),
        destination: store_path.clone(),
        // These temporary values are not used by inspect.
        expected_source: crate::fs::canonical_transfer::ExpectedCanonicalEntry::unchecked(
            CanonicalEntryKind::RegularFile,
        ),
        expected_destination: crate::fs::canonical_transfer::ExpectedCanonicalEntry::unchecked(
            CanonicalEntryKind::Missing,
        ),
    };
    let facts = transfer.inspect(CanonicalTransferInspectionRequest {
        action,
        purpose: CanonicalInspectionPurpose::Planning,
    })?;
    if facts.source_kind != CanonicalEntryKind::RegularFile || !facts.source_hardlink_free {
        return Err(AppError::HardlinkedFile {
            path: repo_root.join(repo_path.as_str()),
        });
    }
    Ok(facts.expected_source())
}

fn canonical_destination_expectation(
    repo_root: &Path,
    store_root: &Path,
    repo_path: &RepoRelativePath,
    store_path: &StoreRelativePath,
) -> Result<crate::fs::canonical_transfer::ExpectedCanonicalEntry> {
    let transfer = DefaultCanonicalTransfer::new(repo_root.to_path_buf(), store_root.to_path_buf());
    let action = CanonicalTransferAction::ReplaceFromRepo {
        source: repo_path.clone(),
        destination: store_path.clone(),
        expected_source: crate::fs::canonical_transfer::ExpectedCanonicalEntry::unchecked(
            CanonicalEntryKind::RegularFile,
        ),
        expected_destination: crate::fs::canonical_transfer::ExpectedCanonicalEntry::unchecked(
            CanonicalEntryKind::Missing,
        ),
    };
    let facts = transfer.inspect(CanonicalTransferInspectionRequest {
        action,
        purpose: CanonicalInspectionPurpose::Planning,
    })?;
    if facts.destination_kind != CanonicalEntryKind::Missing || !facts.destination_hardlink_free {
        return Err(AppError::StoreConflict {
            store_path: store_root.join(store_path.as_str()),
        });
    }
    Ok(facts.expected_destination())
}

fn validate_add_transfer_preconditions(
    ctx: &RepoContext,
    plan: &ItemAddPlan,
    ignore: &dyn IgnoreBackend,
    facts: &crate::fs::canonical_transfer::CanonicalTransferFacts,
) -> Result<()> {
    if facts.source_kind != CanonicalEntryKind::RegularFile || !facts.source_hardlink_free {
        return Err(AppError::HardlinkedFile {
            path: plan.abs_path.clone(),
        });
    }
    if facts.destination_kind != CanonicalEntryKind::Missing || !facts.destination_hardlink_free {
        return Err(AppError::StoreConflict {
            store_path: plan.store_path.clone(),
        });
    }
    item_validation::validate_add_git_state(
        &plan.abs_path,
        git::is_tracked(&ctx.repo_root, &plan.abs_path)?,
    )?;
    if !ignore.has_entry(&ctx.repo_root, &plan.path)? {
        return Err(AppError::Internal(
            "managed add exclude was removed before transfer".into(),
        ));
    }
    Ok(())
}

fn validate_add_materialization_preconditions(
    ctx: &RepoContext,
    plan: &ItemAddPlan,
    ignore: &dyn IgnoreBackend,
    facts: &crate::fs::materializer::MaterializationFacts,
) -> Result<()> {
    if facts.repo_entry_kind != RepoEntryKind::Missing || !facts.store_exists {
        return Err(AppError::FilesystemEntryChanged {
            path: plan.abs_path.clone(),
        });
    }
    if !ignore.has_entry(&ctx.repo_root, &plan.path)? {
        return Err(AppError::Internal(
            "managed add exclude was removed before materialization".into(),
        ));
    }
    if git::is_tracked(&ctx.repo_root, &plan.abs_path)? {
        return Err(AppError::PathIsTracked {
            path: plan.abs_path.clone(),
        });
    }
    Ok(())
}

fn validate_add_postconditions(
    ctx: &RepoContext,
    plan: &ItemAddPlan,
    ignore: &dyn IgnoreBackend,
    strategy: MaterializationStrategy,
    facts: &crate::fs::materializer::MaterializationFacts,
) -> Result<()> {
    let materialized = match strategy {
        MaterializationStrategy::Symlink => facts.repo_entry_kind == RepoEntryKind::ManagedSymlink,
        MaterializationStrategy::Copy => {
            facts.repo_entry_kind == RepoEntryKind::RegularFile
                && facts.copy_content == crate::domain::materialization::CopyContentState::Equal
                && facts.hardlink_free
        }
    };
    if !materialized || !ignore.has_entry(&ctx.repo_root, &plan.path)? {
        return Err(AppError::Internal(
            "add postconditions failed; durable recovery record was retained".into(),
        ));
    }
    if git::is_tracked(&ctx.repo_root, &plan.abs_path)? {
        return Err(AppError::PathIsTracked {
            path: plan.abs_path.clone(),
        });
    }
    Ok(())
}

// ── Directory namespace shelving ───────────────────────────────────────────────

/// Why a candidate file was skipped during a directory add.
#[derive(Debug)]
pub enum SkipReason {
    /// Already recorded in the shelfbox manifest.
    AlreadyManaged,
    /// Tracked by git; shelving is refused.
    GitTracked,
    /// Already a symlink; shelving symlinks is not supported.
    IsSymlink,
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkipReason::AlreadyManaged => write!(f, "already managed by shelfbox"),
            SkipReason::GitTracked => write!(f, "tracked by git"),
            SkipReason::IsSymlink => write!(f, "is a symlink"),
        }
    }
}

/// Outcome for a single file processed during directory add.
#[derive(Debug)]
pub enum DirItemOutcome {
    /// File was successfully shelved.
    Added,
    /// File would be shelved (dry-run mode).
    WouldAdd,
    /// File was skipped with a benign reason.
    Skipped(SkipReason),
    /// A nested git repository was found and its contents were excluded.
    NestedGitRepo,
    /// Shelving failed with an error.
    Failed(String),
}

/// Summary of a directory add operation.
#[derive(Debug)]
pub struct DirectoryAddResult {
    /// Directory path that was processed (repo-relative, ends with `/`).
    pub ns_path: String,
    /// Per-file outcomes in the order they were processed.
    pub results: Vec<(String, DirItemOutcome)>,
    /// Always false in v0.7.0; namespaces are UI-only and not persisted.
    pub namespace_created: bool,
}

/// Shelves all eligible files under `abs_dir`.
///
/// Each eligible file is moved to the store and replaced with a symlink.
///
/// # Eligibility rules
///
/// A file is eligible if it is:
/// - not already managed by shelfbox,
/// - not tracked by git,
/// - not a symlink.
///
/// Nested git repositories inside `abs_dir` are reported as
/// [`DirItemOutcome::NestedGitRepo`] and their contents are excluded entirely.
/// Partial success is allowed.
///
/// # Dry-run
/// When `dry_run` is `true`, no filesystem changes are made.
pub fn add_directory(
    ctx: &mut RepoContext,
    abs_dir: &Path,
    dry_run: bool,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<DirectoryAddResult> {
    // ── Validate the directory ───────────────────────────────────────────────
    let rel_dir = repo_relative_path(&ctx.repo_root, abs_dir)?;
    let rel_str = repo_relative_string(&ctx.repo_root, abs_dir)?;

    item_validation::validate_add_location(&rel_dir, abs_dir)?;

    // Namespace path always ends with "/" for unambiguous prefix matching.
    let ns_path = format!("{rel_str}/");

    // ── Collect candidates ───────────────────────────────────────────────────
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut nested_repos: Vec<PathBuf> = Vec::new();
    collect_dir_candidates(abs_dir, &mut candidates, &mut nested_repos)
        .map_err(|e| AppError::io(abs_dir, e))?;

    // Pre-fetch git-tracked paths in the directory (one subprocess instead of N).
    let tracked = git::tracked_files_in_dir(&ctx.repo_root, abs_dir).unwrap_or_default();

    let mut results: Vec<(String, DirItemOutcome)> = Vec::new();

    // Report nested git repos as non-fatal blocking entries.
    for nested in &nested_repos {
        let rel_buf =
            repo_relative_path(&ctx.repo_root, nested).unwrap_or_else(|_| nested.to_path_buf());
        let rel = rel_buf.as_path();
        results.push((normalize_repo_relative(rel), DirItemOutcome::NestedGitRepo));
    }

    // ── Process each candidate ───────────────────────────────────────────────
    let mut to_shelve: Vec<(String, PathBuf)> = Vec::new(); // (rel, abs)

    for candidate in candidates {
        let rel_cand = repo_relative_path(&ctx.repo_root, &candidate)?;
        let rel_cand_str = normalize_repo_relative(&rel_cand);

        let meta = match candidate.symlink_metadata() {
            Ok(m) => m,
            Err(e) => {
                results.push((
                    rel_cand_str,
                    DirItemOutcome::Failed(format!("failed to stat: {e}")),
                ));
                continue;
            }
        };
        let store_path = ctx.store_path_for(&rel_cand_str);
        let decision = item_validation::classify_directory_candidate(
            ctx.manifest.contains(&rel_cand_str),
            meta.file_type().is_symlink(),
            tracked.contains(&rel_cand_str),
            store_path.exists(),
        );
        match decision {
            DirectoryCandidateDecision::Add => {}
            DirectoryCandidateDecision::SkipAlreadyManaged => {
                results.push((
                    rel_cand_str,
                    DirItemOutcome::Skipped(SkipReason::AlreadyManaged),
                ));
                continue;
            }
            DirectoryCandidateDecision::SkipGitTracked => {
                results.push((
                    rel_cand_str,
                    DirItemOutcome::Skipped(SkipReason::GitTracked),
                ));
                continue;
            }
            DirectoryCandidateDecision::SkipSymlink => {
                results.push((rel_cand_str, DirItemOutcome::Skipped(SkipReason::IsSymlink)));
                continue;
            }
            DirectoryCandidateDecision::StoreConflict => {
                results.push((
                    rel_cand_str,
                    DirItemOutcome::Failed(item_validation::conflict_message(store_path)),
                ));
                continue;
            }
        }

        to_shelve.push((rel_cand_str, candidate));
    }

    // ── Dry-run ──────────────────────────────────────────────────────────────
    if dry_run {
        for (rel, _) in &to_shelve {
            results.push((rel.clone(), DirItemOutcome::WouldAdd));
        }
        return Ok(DirectoryAddResult {
            ns_path,
            results,
            namespace_created: false,
        });
    }

    // ── Execute shelving ─────────────────────────────────────────────────────
    // Each candidate owns an independent durable add record. This preserves
    // the established partial-success behavior without allowing one file's
    // interruption to leave another file outside the recovery protocol.
    for (rel_cand_str, abs_cand) in to_shelve {
        match add_report(ctx, &abs_cand, false, link, ignore) {
            Ok(_) => results.push((rel_cand_str, DirItemOutcome::Added)),
            Err(error) => results.push((rel_cand_str, DirItemOutcome::Failed(error.to_string()))),
        }
    }

    Ok(DirectoryAddResult {
        ns_path,
        results,
        namespace_created: false,
    })
}

/// Recursively collects file candidates from `dir`.
///
/// Directories that contain a `.git` entry are recorded in `nested_repos`
/// and not descended into.  Symlinks to directories are treated as file
/// candidates (not traversed).
fn collect_dir_candidates(
    dir: &Path,
    candidates: &mut Vec<PathBuf>,
    nested_repos: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // Use file_type() which does NOT follow symlinks.
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            // Refuse to cross a nested git repository boundary.
            if path.join(".git").exists() {
                nested_repos.push(path);
            } else {
                collect_dir_candidates(&path, candidates, nested_repos)?;
            }
        } else {
            // Regular files and symlinks are both collected here.
            // The symlink check is done in add_directory() to report them
            // as SkipReason::IsSymlink rather than silently ignoring.
            candidates.push(path);
        }
    }
    Ok(())
}
