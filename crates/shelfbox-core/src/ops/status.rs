use std::{io::ErrorKind, path::Path};

use serde::Serialize;

use crate::{
    context::RepoContext, error::Result, git, ignore::IgnoreBackend, link::LinkStrategy,
    store::manifest::Item,
};

pub use crate::domain::materialization::MaterializationStrategy;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusSeverity {
    Healthy,
    Warning,
    Error,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusIssueCode {
    MaterializationMissing,
    MaterializationInvalid,
    StoreMissing,
    MissingExclude,
    TrackedByGit,
    ContentDiverged,
    ContentUnreadable,
    HardlinkUnsafe,
    PathEscape,
    UnfinishedOperationConflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusNoteCode {
    StrategyMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusIssue {
    pub code: StatusIssueCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusNote {
    pub code: StatusNoteCode,
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
    ctx.manifest
        .items
        .iter()
        .map(|item| check_item_facts(ctx, item, link, ignore).map(|facts| facts.to_legacy()))
        .collect()
}

/// Returns schema-v2 status DTOs for copy-aware callers.
pub fn status_v2(
    ctx: &RepoContext,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
    options: StatusOptions,
) -> Result<Vec<ItemStatusV2>> {
    match options.schema_version {
        StatusSchemaVersion::V2 => ctx
            .manifest
            .items
            .iter()
            .map(|item| check_item_facts(ctx, item, link, ignore).map(|facts| facts.to_v2()))
            .collect(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusFacts {
    path: String,
    configured_strategy: MaterializationStrategy,
    observed_materialization: ObservedMaterialization,
    link_exists: bool,
    link_valid: bool,
    store_exists: bool,
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
        let materialization_exists =
            self.observed_materialization != ObservedMaterialization::Missing;
        let materialization_valid = match self.configured_strategy {
            MaterializationStrategy::Symlink => {
                self.observed_materialization == ObservedMaterialization::ManagedSymlink
            }
            MaterializationStrategy::Copy => {
                self.observed_materialization == ObservedMaterialization::RegularFile
            }
        };
        let evaluation = evaluate_status(self, materialization_exists, materialization_valid);

        ItemStatusV2 {
            status_schema_version: STATUS_SCHEMA_VERSION_V2,
            path: self.path.clone(),
            configured_strategy: self.configured_strategy,
            observed_materialization: self.observed_materialization,
            materialization_exists,
            materialization_valid,
            content_state: CopyContentState::NotApplicable,
            store_exists: self.store_exists,
            in_exclude: self.in_exclude,
            not_tracked: self.not_tracked,
            severity: evaluation.severity,
            issues: evaluation.issues,
            notes: Vec::new(),
            ok: evaluation.ok,
            link_exists: Some(self.link_exists),
            link_valid: Some(self.link_valid),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusEvaluation {
    severity: StatusSeverity,
    issues: Vec<StatusIssue>,
    ok: bool,
}

fn evaluate_status(
    facts: &StatusFacts,
    materialization_exists: bool,
    materialization_valid: bool,
) -> StatusEvaluation {
    let mut issues = Vec::new();

    if !materialization_exists {
        issues.push(StatusIssue {
            code: StatusIssueCode::MaterializationMissing,
        });
    } else if !materialization_valid {
        issues.push(StatusIssue {
            code: StatusIssueCode::MaterializationInvalid,
        });
    }
    if !facts.store_exists {
        issues.push(StatusIssue {
            code: StatusIssueCode::StoreMissing,
        });
    }
    if !facts.in_exclude {
        issues.push(StatusIssue {
            code: StatusIssueCode::MissingExclude,
        });
    }
    if !facts.not_tracked {
        issues.push(StatusIssue {
            code: StatusIssueCode::TrackedByGit,
        });
    }

    let severity = if !materialization_valid || !facts.store_exists || !facts.not_tracked {
        StatusSeverity::Error
    } else if !facts.in_exclude {
        StatusSeverity::Warning
    } else {
        StatusSeverity::Healthy
    };

    StatusEvaluation {
        severity,
        issues,
        ok: severity == StatusSeverity::Healthy,
    }
}

fn check_item_facts(
    ctx: &RepoContext,
    item: &Item,
    link: &dyn LinkStrategy,
    ignore: &dyn IgnoreBackend,
) -> Result<StatusFacts> {
    let abs_path = ctx.repo_root.join(&item.path);
    let store_path = ctx.repo_store.join(&item.store_path);

    // Does a symlink exist at the repo-side path (via the link strategy)?
    let link_exists = link.is_link(&abs_path);
    // Is it specifically a managed symlink pointing into the store?
    let link_valid = link.is_managed_link(&abs_path, &ctx.config.store);
    // Does the store-side copy exist?
    let store_exists = store_path.exists();
    // Is the path listed in .git/info/exclude?
    let in_exclude = ignore.has_entry(&ctx.repo_root, &item.path)?;
    // Is the path not tracked by Git (i.e. not accidentally staged as the symlink)?
    let not_tracked = !git::is_tracked(&ctx.repo_root, &abs_path)?;

    Ok(StatusFacts {
        path: item.path.clone(),
        configured_strategy: MaterializationStrategy::Symlink,
        observed_materialization: observed_materialization(&abs_path, link_exists, link_valid),
        link_exists,
        link_valid,
        store_exists,
        in_exclude,
        not_tracked,
    })
}

fn observed_materialization(
    abs_path: &Path,
    link_exists: bool,
    link_valid: bool,
) -> ObservedMaterialization {
    let metadata = match std::fs::symlink_metadata(abs_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return ObservedMaterialization::Missing;
        }
        Err(_) => return ObservedMaterialization::Other,
    };
    let file_type = metadata.file_type();

    if file_type.is_symlink() {
        if link_valid {
            ObservedMaterialization::ManagedSymlink
        } else if link_exists {
            ObservedMaterialization::UnmanagedSymlink
        } else {
            ObservedMaterialization::Other
        }
    } else if file_type.is_file() {
        ObservedMaterialization::RegularFile
    } else if file_type.is_dir() {
        ObservedMaterialization::Directory
    } else {
        ObservedMaterialization::Other
    }
}

#[cfg(test)]
pub(crate) fn healthy_symlink_status_v2_fixture() -> ItemStatusV2 {
    healthy_symlink_facts().to_v2()
}

#[cfg(test)]
pub(crate) fn copy_item_status_v2_fixture() -> ItemStatusV2 {
    ItemStatusV2 {
        status_schema_version: STATUS_SCHEMA_VERSION_V2,
        path: "copy.txt".into(),
        configured_strategy: MaterializationStrategy::Copy,
        observed_materialization: ObservedMaterialization::RegularFile,
        materialization_exists: true,
        materialization_valid: true,
        content_state: CopyContentState::Equal,
        store_exists: true,
        in_exclude: true,
        not_tracked: true,
        severity: StatusSeverity::Healthy,
        issues: Vec::new(),
        notes: Vec::new(),
        ok: true,
        link_exists: None,
        link_valid: None,
    }
}

#[cfg(test)]
fn healthy_symlink_facts() -> StatusFacts {
    StatusFacts {
        path: "secret.txt".into(),
        configured_strategy: MaterializationStrategy::Symlink,
        observed_materialization: ObservedMaterialization::ManagedSymlink,
        link_exists: true,
        link_valid: true,
        store_exists: true,
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
            store_exists: false,
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
                    {"code": "missing_exclude"},
                    {"code": "tracked_by_git"}
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
