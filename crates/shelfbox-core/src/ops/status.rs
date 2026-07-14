use serde::Serialize;

use crate::{
    context::RepoContext,
    domain::materialization::{
        CopyContentState as DomainCopyContentState, ExcludeState, GitState,
        MaterializationFacts as DomainMaterializationFacts, MaterializationRelation,
        RepoEntryKind as DomainRepoEntryKind, StoreState,
    },
    error::Result,
    fs::materializer::{
        DefaultMaterializer, InspectionPurpose, MaterializationInspectionRequest,
        MaterializationLocation, Materializer, RepoEntryKind as FsRepoEntryKind,
    },
    git,
    ignore::IgnoreBackend,
    link::LinkStrategy,
    policy::materialization_policy::evaluate_materialization_status,
    store::manifest::Item,
};

use super::recovery;

pub use crate::domain::materialization::{
    MaterializationStrategy, StatusIssue, StatusIssueCode, StatusNote, StatusNoteCode,
    StatusSeverity,
};

pub const STATUS_SCHEMA_VERSION_V2: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusSchemaVersion {
    V2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusOptions {
    pub schema_version: StatusSchemaVersion,
}

impl StatusOptions {
    pub const fn v2() -> Self {
        Self {
            schema_version: StatusSchemaVersion::V2,
        }
    }
}

/// Legacy symlink-compatibility health status for a single shelved item.
///
/// This type intentionally remains source-compatible with v0.8.0. Copy-aware
/// callers should use [`ItemStatusV2`] instead.
#[derive(Debug, Serialize)]
pub struct ItemStatus {
    /// Repo-relative path of the item.
    pub path: String,
    /// `true` if a filesystem entry exists at the repo-side path
    /// (including dangling symlinks).
    pub link_exists: bool,
    /// `true` if the repo-side path is a managed symlink pointing into the store.
    pub link_valid: bool,
    /// `true` if the store-side path exists on disk.
    pub store_exists: bool,
    /// `true` if the path appears in `.git/info/exclude`.
    pub in_exclude: bool,
    /// `true` when the path is NOT tracked by Git (expected for shelved items).
    pub not_tracked: bool,
    /// `true` when every other field is `true`.
    pub ok: bool,
}

/// Schema-v2 presentation classification retained for source and JSON
/// compatibility. Phase 4 will project the richer domain facts into this
/// compatibility DTO without changing legacy symlink values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedMaterialization {
    Missing,
    ManagedSymlink,
    UnmanagedSymlink,
    RegularFile,
    Directory,
    Other,
}

/// Schema-v2 presentation state retained for the existing compatibility
/// projection. The domain facts model uses its own narrower content-comparison
/// vocabulary, which does not conflate a missing store with comparison state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyContentState {
    NotApplicable,
    Equal,
    Diverged,
    StoreMissing,
    RepoUnreadable,
    StoreUnreadable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemStatusV2 {
    pub status_schema_version: u32,
    pub path: String,
    pub configured_strategy: MaterializationStrategy,
    pub observed_materialization: ObservedMaterialization,
    pub materialization_exists: bool,
    pub materialization_valid: bool,
    pub content_state: CopyContentState,
    pub store_exists: bool,
    pub in_exclude: bool,
    pub not_tracked: bool,
    pub severity: StatusSeverity,
    pub issues: Vec<StatusIssue>,
    pub notes: Vec<StatusNote>,
    pub ok: bool,
    pub link_exists: Option<bool>,
    pub link_valid: Option<bool>,
}

/// Returns the legacy symlink-compatibility health status for every item
/// currently in the manifest.
pub fn status(
    ctx: &RepoContext,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<Vec<ItemStatus>> {
    let materializer = DefaultMaterializer::with_link_strategy(
        ctx.repo_root.clone(),
        ctx.repo_store.clone(),
        link,
    );
    ctx.manifest
        .items
        .iter()
        .map(|item| {
            check_item_facts(ctx, item, &materializer, ignore).map(|facts| facts.to_legacy())
        })
        .collect()
}

/// Returns schema-v2 status DTOs for copy-aware callers.
pub fn status_v2(
    ctx: &RepoContext,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
    options: StatusOptions,
) -> Result<Vec<ItemStatusV2>> {
    let materializer = DefaultMaterializer::with_link_strategy(
        ctx.repo_root.clone(),
        ctx.repo_store.clone(),
        link,
    );
    status_v2_with_materializer(ctx, &materializer, ignore, options)
}

/// Evaluates schema-v2 status with an operation-facing materializer port.
///
/// New Copy-aware operations use this variant so they do not need to depend on
/// the legacy symlink strategy adapter merely to perform read-only inspection.
pub(crate) fn status_v2_with_materializer(
    ctx: &RepoContext,
    materializer: &dyn Materializer,
    ignore: &dyn IgnoreBackend,
    options: StatusOptions,
) -> Result<Vec<ItemStatusV2>> {
    let mut statuses: Vec<ItemStatusV2> = match options.schema_version {
        StatusSchemaVersion::V2 => ctx
            .manifest
            .items
            .iter()
            .map(|item| {
                check_item_facts(ctx, item, materializer, ignore).map(|facts| facts.to_v2())
            })
            .collect::<Result<Vec<_>>>(),
    }?;
    apply_read_only_recovery_status(&mut statuses, ctx);
    Ok(statuses)
}

fn apply_read_only_recovery_status(statuses: &mut [ItemStatusV2], ctx: &RepoContext) {
    let recovery_status = recovery::read_only_status(&ctx.config.store, &ctx.repo_root)
        .unwrap_or_else(|_| recovery::ReadOnlyRecoveryStatus {
            has_unattributed_record: true,
            ..recovery::ReadOnlyRecoveryStatus::default()
        });
    if recovery_status.record_ids.is_empty() && !recovery_status.has_unattributed_record {
        return;
    }

    for status in statuses {
        if recovery_status.has_unattributed_record
            || recovery_status.affected_item_paths.contains(&status.path)
        {
            if !status
                .issues
                .iter()
                .any(|issue| issue.code == StatusIssueCode::UnfinishedOperationConflict)
            {
                status.issues.push(StatusIssue {
                    code: StatusIssueCode::UnfinishedOperationConflict,
                });
            }
            status.severity = StatusSeverity::Error;
            status.ok = false;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusFacts {
    path: String,
    configured_strategy: MaterializationStrategy,
    observed_materialization: ObservedMaterialization,
    link_exists: bool,
    link_valid: bool,
    hardlink_free: bool,
    store_exists: bool,
    content_state: CopyContentState,
    in_exclude: bool,
    not_tracked: bool,
}

impl StatusFacts {
    fn to_legacy(&self) -> ItemStatus {
        ItemStatus {
            path: self.path.clone(),
            link_exists: self.link_exists,
            link_valid: self.link_valid,
            store_exists: self.store_exists,
            in_exclude: self.in_exclude,
            not_tracked: self.not_tracked,
            ok: self.link_exists
                && self.link_valid
                && self.store_exists
                && self.in_exclude
                && self.not_tracked,
        }
    }

    fn to_v2(&self) -> ItemStatusV2 {
        let evaluation =
            evaluate_materialization_status(self.policy_facts(), self.configured_strategy);

        ItemStatusV2 {
            status_schema_version: STATUS_SCHEMA_VERSION_V2,
            path: self.path.clone(),
            configured_strategy: self.configured_strategy,
            observed_materialization: self.observed_materialization,
            materialization_exists: evaluation.materialization_exists,
            materialization_valid: evaluation.materialization_valid,
            content_state: self.content_state,
            store_exists: self.store_exists,
            in_exclude: self.in_exclude,
            not_tracked: self.not_tracked,
            severity: evaluation.severity,
            issues: evaluation.issues,
            notes: evaluation.notes,
            ok: evaluation.severity == StatusSeverity::Healthy,
            link_exists: (self.observed_materialization != ObservedMaterialization::RegularFile)
                .then_some(self.link_exists),
            link_valid: (self.observed_materialization != ObservedMaterialization::RegularFile)
                .then_some(self.link_valid),
        }
    }

    fn policy_facts(&self) -> DomainMaterializationFacts {
        let (repo_entry, relation) = match self.observed_materialization {
            ObservedMaterialization::Missing => (
                DomainRepoEntryKind::Missing,
                MaterializationRelation::NotApplicable,
            ),
            ObservedMaterialization::ManagedSymlink => (
                DomainRepoEntryKind::Symlink,
                MaterializationRelation::ManagedSymlink,
            ),
            ObservedMaterialization::UnmanagedSymlink => (
                DomainRepoEntryKind::Symlink,
                MaterializationRelation::UnexpectedSymlink,
            ),
            ObservedMaterialization::RegularFile => {
                let relation = if self.hardlink_free {
                    MaterializationRelation::IsolatedRegularCopy
                } else {
                    MaterializationRelation::UnsafeHardlink
                };
                (DomainRepoEntryKind::RegularFile, relation)
            }
            ObservedMaterialization::Directory | ObservedMaterialization::Other => (
                DomainRepoEntryKind::Unsupported,
                MaterializationRelation::NotApplicable,
            ),
        };
        DomainMaterializationFacts {
            repo_entry,
            relation,
            copy_content: match self.content_state {
                CopyContentState::NotApplicable | CopyContentState::StoreMissing => {
                    DomainCopyContentState::NotCompared
                }
                CopyContentState::Equal => DomainCopyContentState::Equal,
                CopyContentState::Diverged => DomainCopyContentState::Diverged,
                CopyContentState::RepoUnreadable | CopyContentState::StoreUnreadable => {
                    DomainCopyContentState::Unreadable
                }
                CopyContentState::Unknown => DomainCopyContentState::ComparisonFailed,
            },
            store_state: match (self.store_exists, self.content_state) {
                (false, _) => StoreState::Missing,
                (_, CopyContentState::StoreUnreadable) => StoreState::Unreadable,
                _ => StoreState::Present,
            },
            git_state: if self.not_tracked {
                GitState::Untracked
            } else {
                GitState::Tracked
            },
            exclude_state: if self.in_exclude {
                ExcludeState::Present
            } else {
                ExcludeState::Missing
            },
        }
    }
}

fn check_item_facts(
    ctx: &RepoContext,
    item: &Item,
    materializer: &dyn Materializer,
    ignore: &dyn IgnoreBackend,
) -> Result<StatusFacts> {
    let abs_path = ctx.repo_root.join(&item.path);
    let location = MaterializationLocation::new(
        item.path
            .parse()
            .map_err(|_| crate::error::AppError::Internal("invalid manifest repo path".into()))?,
        item.store_path
            .parse()
            .map_err(|_| crate::error::AppError::Internal("invalid manifest store path".into()))?,
    );
    let inspection = materializer.inspect(MaterializationInspectionRequest {
        location,
        purpose: InspectionPurpose::Planning,
    })?;
    let observed_materialization = observed_materialization(inspection.repo_entry_kind);
    let (link_exists, link_valid) = match inspection.repo_entry_kind {
        FsRepoEntryKind::ManagedSymlink => (true, true),
        FsRepoEntryKind::UnmanagedSymlinkOrReparsePoint => (true, false),
        _ => (false, false),
    };
    let content_state = copy_content_state(
        observed_materialization,
        inspection.store_exists,
        inspection.copy_content,
    );
    // Is the path listed in .git/info/exclude?
    let in_exclude = ignore.has_entry(&ctx.repo_root, &item.path)?;
    // Is the path not tracked by Git (i.e. not accidentally staged as the symlink)?
    let not_tracked = !git::is_tracked(&ctx.repo_root, &abs_path)?;

    Ok(StatusFacts {
        path: item.path.clone(),
        configured_strategy: ctx.config.materialization,
        observed_materialization,
        link_exists,
        link_valid,
        hardlink_free: inspection.hardlink_free,
        store_exists: inspection.store_exists,
        content_state,
        in_exclude,
        not_tracked,
    })
}

fn copy_content_state(
    observed: ObservedMaterialization,
    store_exists: bool,
    inspected: DomainCopyContentState,
) -> CopyContentState {
    if observed != ObservedMaterialization::RegularFile {
        return CopyContentState::NotApplicable;
    }
    if !store_exists {
        return CopyContentState::StoreMissing;
    }

    match inspected {
        DomainCopyContentState::Equal => CopyContentState::Equal,
        DomainCopyContentState::Diverged => CopyContentState::Diverged,
        DomainCopyContentState::Unreadable => CopyContentState::RepoUnreadable,
        DomainCopyContentState::NotCompared | DomainCopyContentState::ComparisonFailed => {
            CopyContentState::Unknown
        }
    }
}

fn observed_materialization(entry_kind: FsRepoEntryKind) -> ObservedMaterialization {
    match entry_kind {
        FsRepoEntryKind::Missing => ObservedMaterialization::Missing,
        FsRepoEntryKind::ManagedSymlink => ObservedMaterialization::ManagedSymlink,
        FsRepoEntryKind::UnmanagedSymlinkOrReparsePoint => {
            ObservedMaterialization::UnmanagedSymlink
        }
        FsRepoEntryKind::RegularFile => ObservedMaterialization::RegularFile,
        FsRepoEntryKind::Directory => ObservedMaterialization::Directory,
        FsRepoEntryKind::Other => ObservedMaterialization::Other,
    }
}

#[cfg(test)]
pub(crate) fn healthy_symlink_status_v2_fixture() -> ItemStatusV2 {
    healthy_symlink_facts().to_v2()
}

#[cfg(test)]
pub(crate) fn copy_item_status_v2_fixture() -> ItemStatusV2 {
    StatusFacts {
        path: "copy.txt".into(),
        configured_strategy: MaterializationStrategy::Copy,
        observed_materialization: ObservedMaterialization::RegularFile,
        content_state: CopyContentState::Equal,
        store_exists: true,
        in_exclude: true,
        not_tracked: true,
        link_exists: false,
        link_valid: false,
        hardlink_free: true,
    }
    .to_v2()
}

#[cfg(test)]
fn healthy_symlink_facts() -> StatusFacts {
    StatusFacts {
        path: "secret.txt".into(),
        configured_strategy: MaterializationStrategy::Symlink,
        observed_materialization: ObservedMaterialization::ManagedSymlink,
        link_exists: true,
        link_valid: true,
        hardlink_free: true,
        store_exists: true,
        content_state: CopyContentState::NotApplicable,
        in_exclude: true,
        not_tracked: true,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn healthy_symlink_v2_json_shape_matches_golden_fixture() {
        let statuses = vec![healthy_symlink_status_v2_fixture()];
        let actual = format!("{}\n", serde_json::to_string_pretty(&statuses).unwrap());

        assert_eq!(
            actual,
            include_str!("../../tests/fixtures/item-status-symlink-v2.json")
        );
    }

    #[test]
    fn copy_v2_json_shape_uses_null_legacy_link_fields() {
        let statuses = vec![copy_item_status_v2_fixture()];
        let actual = format!("{}\n", serde_json::to_string_pretty(&statuses).unwrap());

        assert_eq!(
            actual,
            include_str!("../../tests/fixtures/item-status-copy-v2.json")
        );
    }

    #[test]
    fn legacy_and_v2_status_are_derived_from_same_symlink_facts() {
        let facts = healthy_symlink_facts();
        let legacy = facts.to_legacy();
        let v2 = facts.to_v2();

        assert!(legacy.ok);
        assert_eq!(legacy.path, v2.path);
        assert_eq!(Some(legacy.link_exists), v2.link_exists);
        assert_eq!(Some(legacy.link_valid), v2.link_valid);
        assert_eq!(legacy.store_exists, v2.store_exists);
        assert_eq!(legacy.in_exclude, v2.in_exclude);
        assert_eq!(legacy.not_tracked, v2.not_tracked);
        assert_eq!(v2.ok, v2.severity == StatusSeverity::Healthy);
    }

    #[test]
    fn issue_codes_and_severity_are_snake_case() {
        let facts = StatusFacts {
            path: "secret.txt".into(),
            configured_strategy: MaterializationStrategy::Symlink,
            observed_materialization: ObservedMaterialization::Missing,
            link_exists: false,
            link_valid: false,
            hardlink_free: true,
            store_exists: false,
            content_state: CopyContentState::NotApplicable,
            in_exclude: false,
            not_tracked: false,
        };

        assert_eq!(
            serde_json::to_value(facts.to_v2()).unwrap(),
            json!({
                "status_schema_version": 2,
                "path": "secret.txt",
                "configured_strategy": "symlink",
                "observed_materialization": "missing",
                "materialization_exists": false,
                "materialization_valid": false,
                "content_state": "not_applicable",
                "store_exists": false,
                "in_exclude": false,
                "not_tracked": false,
                "severity": "error",
                "issues": [
                    {"code": "materialization_missing"},
                    {"code": "store_missing"},
                    {"code": "tracked_by_git"},
                    {"code": "missing_exclude"}
                ],
                "notes": [],
                "ok": false,
                "link_exists": false,
                "link_valid": false
            })
        );
    }

    #[test]
    fn all_status_enums_use_frozen_snake_case_names() {
        assert_eq!(
            serde_json::to_value(StatusSeverity::Healthy).unwrap(),
            json!("healthy")
        );
        assert_eq!(
            serde_json::to_value(StatusSeverity::Warning).unwrap(),
            json!("warning")
        );
        assert_eq!(
            serde_json::to_value(StatusSeverity::Error).unwrap(),
            json!("error")
        );

        assert_eq!(
            serde_json::to_value(MaterializationStrategy::Symlink).unwrap(),
            json!("symlink")
        );
        assert_eq!(
            serde_json::to_value(MaterializationStrategy::Copy).unwrap(),
            json!("copy")
        );

        assert_eq!(
            serde_json::to_value(ObservedMaterialization::ManagedSymlink).unwrap(),
            json!("managed_symlink")
        );
        assert_eq!(
            serde_json::to_value(ObservedMaterialization::UnmanagedSymlink).unwrap(),
            json!("unmanaged_symlink")
        );
        assert_eq!(
            serde_json::to_value(ObservedMaterialization::RegularFile).unwrap(),
            json!("regular_file")
        );

        assert_eq!(
            serde_json::to_value(CopyContentState::NotApplicable).unwrap(),
            json!("not_applicable")
        );
        assert_eq!(
            serde_json::to_value(CopyContentState::Diverged).unwrap(),
            json!("diverged")
        );

        assert_eq!(
            serde_json::to_value(StatusIssue {
                code: StatusIssueCode::UnfinishedOperationConflict,
            })
            .unwrap(),
            json!({"code": "unfinished_operation_conflict"})
        );
        assert_eq!(
            serde_json::to_value(StatusNote {
                code: StatusNoteCode::StrategyMismatch,
            })
            .unwrap(),
            json!({"code": "strategy_mismatch"})
        );
    }
}
