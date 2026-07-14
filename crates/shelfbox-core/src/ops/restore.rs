//! Copy-aware restore and legacy detach.
//!
//! A normal restore is a durable lifecycle transition: a managed symlink is
//! first regularized through `Materializer`, the canonical file is staged in
//! an opaque store backup through `CanonicalTransfer`, and only then are
//! manifest and exclude ownership finalized.  An equal regular copy already
//! contains canonical bytes, so it intentionally skips the first write.

use std::path::Path;

use ulid::Ulid;

use crate::{
    context::{self, RepoContext},
    domain::{
        materialization::{CopyContentState, MaterializationStrategy},
        operation_record::{
            ArtifactLocation, OperationKind, OperationPhase, OperationPreState, OperationRecord,
            RecoveryAbsolutePath, RecoveryBackupMetadata, RecoveryRecord, RecoveryRecordKind,
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
            ExpectedCanonicalEntry,
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
    plan::item_restore::{ItemRestoreAction, ItemRestorePlan, ItemRestoreReport},
    storage::operation_record_store,
    store::manifest::{self, OwnershipState},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestoreMaterialization {
    ManagedSymlink,
    EqualCopy,
}

/// Restores `abs_path` as a regular file and removes its canonical store item.
///
/// `restore --keep-store` remains the v0.9.0 legacy detach operation: it
/// changes only ownership state and deliberately leaves the observed symlink
/// or copy, store item, manifest entry, and exclude untouched.
pub fn restore(
    ctx: &mut RepoContext,
    abs_path: &Path,
    dry_run: bool,
    keep_ignore: bool,
    keep_store: bool,
    _link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<ItemRestoreReport> {
    let plan = ItemRestorePlan::build(ctx, abs_path, keep_ignore, keep_store, _link)?;

    if dry_run {
        return Ok(ItemRestoreReport {
            plan,
            dry_run: true,
        });
    }

    match plan.action {
        ItemRestoreAction::DetachKeepStore => execute_legacy_detach(ctx, &plan)?,
        ItemRestoreAction::RestoreFile => execute_restore(ctx, &plan, ignore)?,
    }

    Ok(ItemRestoreReport {
        plan,
        dry_run: false,
    })
}

fn execute_legacy_detach(ctx: &mut RepoContext, plan: &ItemRestorePlan) -> Result<()> {
    // Detach is metadata-only.  In particular it does not call an ignore
    // backend: the old `--keep-ignore` toggle was misleading because a
    // detached symlink/copy remains a managed ignored working-tree entry.
    let now = context::now_iso8601();
    ctx.manifest
        .set_ownership_state(&plan.path, OwnershipState::Detached, &now);
    manifest::save(&ctx.repo_store, &ctx.manifest)?;
    failpoint::after(Failpoint::KeepStoreManifestSaved)
}

fn execute_restore(
    ctx: &mut RepoContext,
    plan: &ItemRestorePlan,
    ignore: &dyn IgnoreBackend,
) -> Result<()> {
    let _item = ctx.manifest.get(&plan.path).ok_or_else(|| {
        AppError::Internal("restore plan item disappeared from the manifest".into())
    })?;
    let repo_path = RepoRelativePath::new(plan.path.clone()).ok_or_else(|| {
        AppError::UnsafeFilesystemEntry {
            path: plan.abs_path.clone(),
            reason: "restore repository path is not normalized",
        }
    })?;
    let store_path = store_relative_path(&ctx.config.store, &plan.store_path)?;
    let location = MaterializationLocation::new(repo_path.clone(), store_path.clone());
    let materialization = inspect_restore_materialization(ctx, &location, &plan.abs_path)?;

    if git::is_tracked(&ctx.repo_root, &plan.abs_path)? {
        return Err(AppError::PathIsTracked {
            path: plan.abs_path.clone(),
        });
    }
    if !ignore.has_entry(&ctx.repo_root, &plan.path)? {
        return Err(AppError::Internal(
            "managed restore exclude is missing; refusing to regularize an unexcluded path".into(),
        ));
    }

    let store_fingerprint = RecoveryFingerprint::from_file(&plan.store_path)?;
    let repo_fingerprint = match materialization {
        RestoreMaterialization::ManagedSymlink => None,
        RestoreMaterialization::EqualCopy => Some(RecoveryFingerprint::from_file(&plan.abs_path)?),
    };
    let record_id = Ulid::new().to_string();
    let backup_store_path = StoreRelativePath::new(format!("recovery/{record_id}.restore"))
        .ok_or_else(|| {
            AppError::Internal("generated restore backup path was not normalized".into())
        })?;
    let backup_path = ctx.config.store.join(backup_store_path.as_str());
    let backup = RecoveryBackupMetadata {
        artifact_record_id: Ulid::new().to_string(),
        location: ArtifactLocation::Store {
            path: backup_store_path.clone(),
        },
        expected_identity: None,
        fingerprint: None,
    };
    let strategy = match materialization {
        RestoreMaterialization::ManagedSymlink => MaterializationStrategy::Symlink,
        RestoreMaterialization::EqualCopy => MaterializationStrategy::Copy,
    };
    let mut record = RecoveryRecord {
        schema_version: OPERATION_RECORD_SCHEMA_VERSION,
        record_id,
        created_at: context::now_iso8601(),
        record: RecoveryRecordKind::Operation(OperationRecord {
            operation: OperationKind::Restore,
            phase: OperationPhase::RecordCreated,
            repo_id: ctx.repo_id.clone(),
            repo_root: RecoveryAbsolutePath::new(&ctx.repo_root).ok_or_else(|| {
                AppError::UnsafeFilesystemEntry {
                    path: ctx.repo_root.clone(),
                    reason: "repository root is not a safe absolute path",
                }
            })?,
            repo_store_path: Some(store_relative_path(&ctx.config.store, &ctx.repo_store)?),
            strategy,
            direction: None,
            pre_state: OperationPreState {
                repo_path: Some(repo_path.clone()),
                store_path: Some(store_path.clone()),
                repo_fingerprint,
                store_fingerprint: Some(store_fingerprint.clone()),
                manifest_contains_item: Some(true),
                exclude_owned: Some(true),
                final_exclude_owned: Some(plan.keep_ignore),
            },
            post_state: None,
            artifact_record_ids: Vec::new(),
            backup: Some(backup),
        }),
    };
    let recovery_record_id = record.record_id.clone();
    operation_record_store::create(&ctx.config.store, &record)?;

    let store_root = ctx.config.store.clone();
    let repo_root = ctx.repo_root.clone();
    let mut journal = AddMutationJournal::new(
        &store_root,
        &repo_root,
        ignore,
        &mut record,
        plan.abs_path.clone(),
        backup_path.clone(),
    );
    journal.ensure_store_destination_parent()?;

    let mut materializer = DefaultMaterializer::new(repo_root.clone(), store_root.clone());
    match materialization {
        RestoreMaterialization::ManagedSymlink => {
            let facts = materializer.inspect(MaterializationInspectionRequest {
                location: location.clone(),
                purpose: InspectionPurpose::PreCommit,
            })?;
            ensure_restore_facts(
                &facts,
                &plan.abs_path,
                RestoreMaterialization::ManagedSymlink,
            )?;
            let action = MaterializationAction::RestoreToRegular {
                location: location.clone(),
                expected: facts.expected(),
            };
            let prepared = materializer.prepare(action, &mut journal)?;
            let fresh = materializer.inspect(MaterializationInspectionRequest {
                location: location.clone(),
                purpose: InspectionPurpose::PreCommit,
            })?;
            ensure_restore_facts(
                &fresh,
                &plan.abs_path,
                RestoreMaterialization::ManagedSymlink,
            )?;
            validate_restore_integration(ctx, plan, ignore)?;
            let permit = journal
                .issue_commit_permit(fresh.write_precondition_guard(prepared.commit_context()))?;
            materializer.commit(prepared, permit)?;
        }
        RestoreMaterialization::EqualCopy => {
            let facts = materializer.inspect(MaterializationInspectionRequest {
                location: location.clone(),
                purpose: InspectionPurpose::PreCommit,
            })?;
            ensure_restore_facts(&facts, &plan.abs_path, RestoreMaterialization::EqualCopy)?;
            validate_restore_integration(ctx, plan, ignore)?;
        }
    }
    ensure_regular_matches_store(&plan.abs_path, &plan.store_path, &store_fingerprint)?;
    journal.advance(OperationPhase::RepoRegularized)?;

    let mut transfer = DefaultCanonicalTransfer::new(repo_root.clone(), store_root.clone());
    let inspection = CanonicalTransferAction::Move {
        source: store_path.clone(),
        destination: backup_store_path.clone(),
        expected_source: ExpectedCanonicalEntry::unchecked(CanonicalEntryKind::RegularFile),
        expected_destination: ExpectedCanonicalEntry::unchecked(CanonicalEntryKind::Missing),
    };
    let planning = transfer.inspect(CanonicalTransferInspectionRequest {
        action: inspection,
        purpose: CanonicalInspectionPurpose::Planning,
    })?;
    ensure_restore_transfer_facts(&planning, &plan.store_path, &backup_path)?;
    let action = CanonicalTransferAction::Move {
        source: store_path,
        destination: backup_store_path,
        expected_source: planning.expected_source(),
        expected_destination: planning.expected_destination(),
    };
    let prepared = transfer.prepare(action.clone(), &mut journal)?;
    let facts = transfer.inspect(CanonicalTransferInspectionRequest {
        action: action.clone(),
        purpose: CanonicalInspectionPurpose::PreCommit,
    })?;
    ensure_restore_transfer_facts(&facts, &plan.store_path, &backup_path)?;
    validate_restore_integration(ctx, plan, ignore)?;
    let permit =
        journal.issue_commit_permit(facts.write_precondition_guard(prepared.commit_context()))?;
    transfer.commit(prepared, permit)?;
    let recorded_backup = journal.record_backup_from_path(&backup_path)?;
    journal.advance(OperationPhase::StoreStaged)?;

    ctx.manifest.remove(&plan.path);
    manifest::save(&ctx.repo_store, &ctx.manifest)?;
    journal.advance(OperationPhase::ManifestRemoved)?;

    if !plan.keep_ignore {
        ignore.remove_entries(&ctx.repo_root, &[&plan.path])?;
    }
    if ignore.has_entry(&ctx.repo_root, &plan.path)? != plan.keep_ignore {
        return Err(AppError::Internal(
            "restore exclude finalization did not produce the requested state".into(),
        ));
    }
    journal.advance(OperationPhase::ExcludeUpdated)?;

    ensure_regular_matches_store(&plan.abs_path, &backup_path, &store_fingerprint)?;
    if ctx.manifest.contains(&plan.path) {
        return Err(AppError::Internal(
            "restore manifest postcondition failed; recovery record was retained".into(),
        ));
    }
    journal.cleanup_all()?;
    operation_record_store::cleanup_backup(
        &store_root,
        &repo_root,
        &recovery_record_id,
        &recorded_backup,
    )?;
    journal.advance(OperationPhase::PostCommitValidated)?;
    drop(journal);
    operation_record_store::remove(&store_root, &recovery_record_id)
}

fn validate_restore_integration(
    ctx: &RepoContext,
    plan: &ItemRestorePlan,
    ignore: &dyn IgnoreBackend,
) -> Result<()> {
    if git::is_tracked(&ctx.repo_root, &plan.abs_path)? {
        return Err(AppError::PathIsTracked {
            path: plan.abs_path.clone(),
        });
    }
    if !ignore.has_entry(&ctx.repo_root, &plan.path)? {
        return Err(AppError::Internal(
            "managed restore exclude was removed before commit authorization".into(),
        ));
    }
    Ok(())
}

fn inspect_restore_materialization(
    ctx: &RepoContext,
    location: &MaterializationLocation,
    abs_path: &Path,
) -> Result<RestoreMaterialization> {
    let materializer = DefaultMaterializer::new(ctx.repo_root.clone(), ctx.config.store.clone());
    let facts = materializer.inspect(MaterializationInspectionRequest {
        location: location.clone(),
        purpose: InspectionPurpose::Planning,
    })?;
    ensure_restore_facts(
        &facts,
        abs_path,
        match facts.repo_entry_kind {
            RepoEntryKind::ManagedSymlink => RestoreMaterialization::ManagedSymlink,
            RepoEntryKind::RegularFile => RestoreMaterialization::EqualCopy,
            _ => {
                return Err(AppError::NotManagedLink {
                    path: abs_path.to_path_buf(),
                })
            }
        },
    )?;
    match facts.repo_entry_kind {
        RepoEntryKind::ManagedSymlink => Ok(RestoreMaterialization::ManagedSymlink),
        RepoEntryKind::RegularFile => match facts.copy_content {
            CopyContentState::Equal => Ok(RestoreMaterialization::EqualCopy),
            CopyContentState::Diverged => Err(AppError::ContentDivergedRequiresSync {
                path: abs_path.to_path_buf(),
            }),
            _ => Err(AppError::UnsafeFilesystemEntry {
                path: abs_path.to_path_buf(),
                reason: "restore regular copy could not be compared to canonical content",
            }),
        },
        _ => unreachable!(),
    }
}

fn ensure_restore_facts(
    facts: &crate::fs::materializer::MaterializationFacts,
    abs_path: &Path,
    expected: RestoreMaterialization,
) -> Result<()> {
    if !facts.store_exists || !facts.store_regular {
        return Err(AppError::StoreMissing {
            path: abs_path.to_path_buf(),
            store_path: abs_path.to_path_buf(),
        });
    }
    if !facts.store_hardlink_free || !facts.hardlink_free {
        return Err(AppError::HardlinkedFile {
            path: abs_path.to_path_buf(),
        });
    }
    match expected {
        RestoreMaterialization::ManagedSymlink
            if facts.repo_entry_kind == RepoEntryKind::ManagedSymlink =>
        {
            Ok(())
        }
        RestoreMaterialization::EqualCopy
            if facts.repo_entry_kind == RepoEntryKind::RegularFile
                && facts.copy_content == CopyContentState::Equal =>
        {
            Ok(())
        }
        _ => Err(AppError::FilesystemEntryChanged {
            path: abs_path.to_path_buf(),
        }),
    }
}

fn ensure_restore_transfer_facts(
    facts: &crate::fs::canonical_transfer::CanonicalTransferFacts,
    store_path: &Path,
    backup_path: &Path,
) -> Result<()> {
    if facts.source_kind != CanonicalEntryKind::RegularFile || !facts.source_hardlink_free {
        return Err(AppError::UnsafeFilesystemEntry {
            path: store_path.to_path_buf(),
            reason: "restore canonical source is not an isolated regular file",
        });
    }
    if facts.destination_kind != CanonicalEntryKind::Missing {
        return Err(AppError::FilesystemEntryChanged {
            path: backup_path.to_path_buf(),
        });
    }
    Ok(())
}

fn ensure_regular_matches_store(
    repo_path: &Path,
    store_path: &Path,
    expected: &RecoveryFingerprint,
) -> Result<()> {
    if RecoveryFingerprint::from_file(repo_path)? != *expected
        || RecoveryFingerprint::from_file(store_path)? != *expected
    {
        return Err(AppError::FilesystemEntryChanged {
            path: repo_path.to_path_buf(),
        });
    }
    Ok(())
}

fn store_relative_path(store_root: &Path, absolute: &Path) -> Result<StoreRelativePath> {
    let relative =
        absolute
            .strip_prefix(store_root)
            .map_err(|_| AppError::UnsafeFilesystemEntry {
                path: absolute.to_path_buf(),
                reason: "store path escapes the configured store root",
            })?;
    StoreRelativePath::new(relative.to_string_lossy().replace('\\', "/")).ok_or_else(|| {
        AppError::UnsafeFilesystemEntry {
            path: absolute.to_path_buf(),
            reason: "store path is not normalized",
        }
    })
}

// ── Directory namespace restore ───────────────────────────────────────────────

/// Outcome for a single item during namespace restore.
#[derive(Debug)]
pub enum NsRestoreItemOutcome {
    Restored,
    WouldRestore,
    Failed(String),
}

#[derive(Debug)]
pub struct NamespaceRestoreResult {
    pub ns_path: String,
    pub results: Vec<(String, NsRestoreItemOutcome)>,
    pub namespace_removed: bool,
}

/// Each namespace member enters the same durable per-item workflow.  Earlier
/// successful members are intentionally retained when a later member fails.
pub fn restore_namespace(
    ctx: &mut RepoContext,
    ns_path: &str,
    dry_run: bool,
    keep_ignore: bool,
    keep_store: bool,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<NamespaceRestoreResult> {
    let member_paths: Vec<String> = ctx
        .manifest
        .items
        .iter()
        .filter(|item| item.path.starts_with(ns_path))
        .map(|item| item.path.clone())
        .collect();
    if member_paths.is_empty() {
        return Err(AppError::NamespaceNotFound {
            path: ns_path.to_owned(),
        });
    }

    if dry_run {
        return Ok(NamespaceRestoreResult {
            ns_path: ns_path.to_owned(),
            results: member_paths
                .into_iter()
                .map(|path| (path, NsRestoreItemOutcome::WouldRestore))
                .collect(),
            namespace_removed: false,
        });
    }

    let mut results = Vec::new();
    for member_path in member_paths {
        let abs_path = ctx.repo_root.join(&member_path);
        match restore(ctx, &abs_path, false, keep_ignore, keep_store, link, ignore) {
            Ok(_) => results.push((member_path, NsRestoreItemOutcome::Restored)),
            Err(error) => {
                results.push((member_path, NsRestoreItemOutcome::Failed(error.to_string())))
            }
        }
    }
    Ok(NamespaceRestoreResult {
        ns_path: ns_path.to_owned(),
        results,
        namespace_removed: false,
    })
}
