//! Store-wide, read-only integrity verification.
//!
//! Canonical store contents are authoritative and are therefore checked for
//! every manifest discovered in the store, independently of `index.json`.
//! An index association is used only to add best-effort, copy-aware inspection
//! of an available local repository.  This separation means a stale clone or
//! an unreadable index can never hide a missing canonical store item.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{
    context,
    domain::{
        materialization::{StatusIssueCode, StatusSeverity},
        path::StoreRelativePath,
    },
    error::Result,
    fs::materializer::DefaultMaterializer,
    ignore::GitInfoExclude,
    store::{
        index, manifest,
        scanner::{self, ScanError},
    },
};

use super::status::{self, StatusOptions};

/// The severity of an issue found by [`verify`].
///
/// Both warning and error findings make the command unsuccessful.  The
/// distinction is retained so callers can distinguish unavailable local
/// evidence from an integrity failure in canonical store data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreVerifySeverity {
    Warning,
    Error,
}

impl StoreVerifySeverity {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Warning => "WARNING",
            Self::Error => "ERROR",
        }
    }
}

/// Stable machine-readable classification for a store verification finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "code", content = "detail", rename_all = "snake_case")]
pub enum StoreVerifyIssueCode {
    StoreScanReadFailed,
    StoreScanParseFailed,
    DuplicateRepoId,
    DuplicateItemId,
    IndexUnreadable,
    ManifestInvalid,
    CanonicalStoreMissing,
    CanonicalStoreInvalid,
    CanonicalStoreUnreadable,
    LocalRepositoryUnavailable,
    LocalRepositoryAssociationMismatch,
    LocalInspectionFailed,
    LocalMaterialization(StatusIssueCode),
}

/// One independently actionable finding from a store verification run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoreVerifyIssue {
    pub severity: StoreVerifySeverity,
    pub code: StoreVerifyIssueCode,
    pub repo_id: Option<String>,
    pub repo_store_dir: Option<String>,
    pub item_path: Option<String>,
    pub filesystem_path: Option<PathBuf>,
    pub message: String,
}

/// Store-wide integrity result.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct StoreVerifyReport {
    /// Number of manifest directories discovered by the store scanner.
    pub repositories_scanned: usize,
    /// Number of manifest items whose canonical store paths were checked.
    pub canonical_items_checked: usize,
    /// Every warning and error found during the read-only inspection.
    pub issues: Vec<StoreVerifyIssue>,
}

impl StoreVerifyReport {
    pub fn is_healthy(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn has_errors(&self) -> bool {
        self.issues
            .iter()
            .any(|issue| issue.severity == StoreVerifySeverity::Error)
    }
}

/// Verifies canonical store data and, when safely available, associated local
/// materializations without creating or modifying any state.
pub fn verify(store_root: &Path) -> Result<StoreVerifyReport> {
    let scan = scanner::scan(store_root)?;
    let mut report = StoreVerifyReport {
        repositories_scanned: scan.entries.len(),
        ..StoreVerifyReport::default()
    };

    for error in &scan.errors {
        report.issues.push(scan_issue(error));
    }

    // An unreadable index must not stop canonical verification.  We simply
    // cannot make a trustworthy local association for this pass.
    let loaded_index = match index::load(store_root) {
        Ok(index) => Some(index),
        Err(error) => {
            report.issues.push(issue(
                StoreVerifySeverity::Error,
                StoreVerifyIssueCode::IndexUnreadable,
                None,
                None,
                None,
                Some(index::index_path(store_root)),
                format!("cannot read index: {error}"),
            ));
            None
        }
    };

    for scanned in scan.entries {
        let repo_store = store_root.join("repos").join(&scanned.repo_store_dir);
        // `scanner` intentionally records parse failures rather than failing
        // early.  Re-load through the canonical manifest API here so version
        // and path-safety validation are part of verify's contract too.
        let manifest = match manifest::load(&repo_store) {
            Ok(manifest) => manifest,
            Err(error) => {
                report.issues.push(issue(
                    StoreVerifySeverity::Error,
                    StoreVerifyIssueCode::ManifestInvalid,
                    Some(scanned.manifest.repo_id),
                    Some(scanned.repo_store_dir),
                    None,
                    Some(manifest::manifest_path(&repo_store)),
                    format!("manifest is invalid: {error}"),
                ));
                continue;
            }
        };

        for item in &manifest.items {
            let Some(store_path) = safe_store_item_path(&item.store_path) else {
                report.issues.push(issue(
                    StoreVerifySeverity::Error,
                    StoreVerifyIssueCode::ManifestInvalid,
                    Some(manifest.repo_id.clone()),
                    Some(scanned.repo_store_dir.clone()),
                    Some(item.path.clone()),
                    None,
                    format!(
                        "item '{}' has an unsafe canonical store path '{}'",
                        item.item_id, item.store_path
                    ),
                ));
                continue;
            };
            report.canonical_items_checked += 1;
            check_canonical_store_item(
                &mut report,
                &manifest.repo_id,
                &scanned.repo_store_dir,
                item,
                &repo_store.join(store_path),
            );
        }

        if let Some(index) = loaded_index.as_ref() {
            verify_associated_local_repo(
                &mut report,
                store_root,
                &repo_store,
                &scanned.repo_store_dir,
                &manifest,
                index,
            );
        }
    }

    report.issues.sort_by(|left, right| {
        left.repo_store_dir
            .cmp(&right.repo_store_dir)
            .then_with(|| left.item_path.cmp(&right.item_path))
            .then_with(|| left.message.cmp(&right.message))
    });
    Ok(report)
}

fn safe_store_item_path(value: &str) -> Option<&str> {
    let path = value.strip_prefix("items/")?;
    StoreRelativePath::new(path).is_some().then_some(value)
}

fn check_canonical_store_item(
    report: &mut StoreVerifyReport,
    repo_id: &str,
    repo_store_dir: &str,
    item: &crate::store::manifest::Item,
    store_path: &Path,
) {
    match std::fs::symlink_metadata(store_path) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => report.issues.push(issue(
            StoreVerifySeverity::Error,
            StoreVerifyIssueCode::CanonicalStoreInvalid,
            Some(repo_id.to_string()),
            Some(repo_store_dir.to_string()),
            Some(item.path.clone()),
            Some(store_path.to_path_buf()),
            "canonical store item is not a regular file".into(),
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => report.issues.push(issue(
            StoreVerifySeverity::Error,
            StoreVerifyIssueCode::CanonicalStoreMissing,
            Some(repo_id.to_string()),
            Some(repo_store_dir.to_string()),
            Some(item.path.clone()),
            Some(store_path.to_path_buf()),
            "canonical store item is missing".into(),
        )),
        Err(error) => report.issues.push(issue(
            StoreVerifySeverity::Error,
            StoreVerifyIssueCode::CanonicalStoreUnreadable,
            Some(repo_id.to_string()),
            Some(repo_store_dir.to_string()),
            Some(item.path.clone()),
            Some(store_path.to_path_buf()),
            format!("cannot inspect canonical store item: {error}"),
        )),
    }
}

fn verify_associated_local_repo(
    report: &mut StoreVerifyReport,
    store_root: &Path,
    repo_store: &Path,
    repo_store_dir: &str,
    manifest: &crate::store::manifest::Manifest,
    index: &crate::store::index::Index,
) {
    let Some(entry) = index.get(&manifest.repo_id) else {
        return;
    };
    let Some(root) = entry.root.as_deref() else {
        return;
    };

    if entry.repo_store_dir != repo_store_dir {
        report.issues.push(issue(
            StoreVerifySeverity::Warning,
            StoreVerifyIssueCode::LocalRepositoryAssociationMismatch,
            Some(manifest.repo_id.clone()),
            Some(repo_store_dir.to_string()),
            None,
            Some(root.to_path_buf()),
            "index association points at a different repository store directory".into(),
        ));
        return;
    }

    let read_only = match context::build_read_only(root, Some(store_root)) {
        Ok(read_only) => read_only,
        Err(error) => {
            report.issues.push(issue(
                StoreVerifySeverity::Warning,
                StoreVerifyIssueCode::LocalRepositoryUnavailable,
                Some(manifest.repo_id.clone()),
                Some(repo_store_dir.to_string()),
                None,
                Some(root.to_path_buf()),
                format!("cannot inspect associated local repository: {error}"),
            ));
            return;
        }
    };
    let Some(ctx) = read_only.repo else {
        report.issues.push(issue(
            StoreVerifySeverity::Warning,
            StoreVerifyIssueCode::LocalRepositoryAssociationMismatch,
            Some(manifest.repo_id.clone()),
            Some(repo_store_dir.to_string()),
            None,
            Some(root.to_path_buf()),
            "associated local repository does not resolve to this store manifest".into(),
        ));
        return;
    };
    if ctx.repo_id != manifest.repo_id || ctx.repo_store != repo_store {
        report.issues.push(issue(
            StoreVerifySeverity::Warning,
            StoreVerifyIssueCode::LocalRepositoryAssociationMismatch,
            Some(manifest.repo_id.clone()),
            Some(repo_store_dir.to_string()),
            None,
            Some(root.to_path_buf()),
            "associated local repository resolved to different store data".into(),
        ));
        return;
    }

    let ignore = GitInfoExclude;
    let materializer = DefaultMaterializer::new(ctx.repo_root.clone(), ctx.repo_store.clone());
    let statuses = match status::status_v2_with_materializer(
        &ctx,
        &materializer,
        &ignore,
        StatusOptions::v2(),
    ) {
        Ok(statuses) => statuses,
        Err(error) => {
            report.issues.push(issue(
                StoreVerifySeverity::Error,
                StoreVerifyIssueCode::LocalInspectionFailed,
                Some(manifest.repo_id.clone()),
                Some(repo_store_dir.to_string()),
                None,
                Some(root.to_path_buf()),
                format!("cannot inspect local materialization: {error}"),
            ));
            return;
        }
    };

    for status in statuses {
        if status.severity == StatusSeverity::Healthy {
            continue;
        }
        let severity = match status.severity {
            StatusSeverity::Healthy => unreachable!("healthy statuses were filtered"),
            StatusSeverity::Warning => StoreVerifySeverity::Warning,
            StatusSeverity::Error => StoreVerifySeverity::Error,
        };
        if status.issues.is_empty() {
            report.issues.push(issue(
                severity,
                StoreVerifyIssueCode::LocalInspectionFailed,
                Some(manifest.repo_id.clone()),
                Some(repo_store_dir.to_string()),
                Some(status.path.clone()),
                Some(root.join(&status.path)),
                "local materialization is unhealthy without a reported issue code".into(),
            ));
        } else {
            for local_issue in status.issues {
                report.issues.push(issue(
                    severity,
                    StoreVerifyIssueCode::LocalMaterialization(local_issue.code),
                    Some(manifest.repo_id.clone()),
                    Some(repo_store_dir.to_string()),
                    Some(status.path.clone()),
                    Some(root.join(&status.path)),
                    format!(
                        "local materialization issue: {}",
                        status_issue_label(local_issue.code)
                    ),
                ));
            }
        }
    }
}

fn scan_issue(error: &ScanError) -> StoreVerifyIssue {
    match error {
        ScanError::ReadFailed { dir, source } => issue(
            StoreVerifySeverity::Error,
            StoreVerifyIssueCode::StoreScanReadFailed,
            None,
            Some(dir.clone()),
            None,
            None,
            format!("cannot read repository store: {source}"),
        ),
        ScanError::ParseFailed { dir, source } => issue(
            StoreVerifySeverity::Error,
            StoreVerifyIssueCode::StoreScanParseFailed,
            None,
            Some(dir.clone()),
            None,
            None,
            format!("cannot parse manifest: {source}"),
        ),
        ScanError::DuplicateRepoId { repo_id, dirs } => issue(
            StoreVerifySeverity::Error,
            StoreVerifyIssueCode::DuplicateRepoId,
            Some(repo_id.clone()),
            None,
            None,
            None,
            format!("duplicate repository identity in {}", dirs.join(", ")),
        ),
        ScanError::DuplicateItemId { item_id, repo_ids } => issue(
            StoreVerifySeverity::Error,
            StoreVerifyIssueCode::DuplicateItemId,
            None,
            None,
            Some(item_id.clone()),
            None,
            format!("duplicate item identity in {}", repo_ids.join(", ")),
        ),
    }
}

fn issue(
    severity: StoreVerifySeverity,
    code: StoreVerifyIssueCode,
    repo_id: Option<String>,
    repo_store_dir: Option<String>,
    item_path: Option<String>,
    filesystem_path: Option<PathBuf>,
    message: String,
) -> StoreVerifyIssue {
    StoreVerifyIssue {
        severity,
        code,
        repo_id,
        repo_store_dir,
        item_path,
        filesystem_path,
        message,
    }
}

fn status_issue_label(code: StatusIssueCode) -> &'static str {
    match code {
        StatusIssueCode::MaterializationMissing => "materialization missing",
        StatusIssueCode::MaterializationInvalid => "materialization invalid",
        StatusIssueCode::StoreMissing => "store missing",
        StatusIssueCode::MissingExclude => "missing exclude entry",
        StatusIssueCode::TrackedByGit => "tracked by Git",
        StatusIssueCode::ContentDiverged => "content diverged",
        StatusIssueCode::ContentUnreadable => "content unreadable",
        StatusIssueCode::HardlinkUnsafe => "unsafe hardlink",
        StatusIssueCode::PathEscape => "path escapes its repository",
        StatusIssueCode::UnfinishedOperationConflict => "unfinished operation conflict",
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::{context, integration_test_common as common};

    fn add_item(repo: &Path, store: &Path, path: &str, contents: &str) -> PathBuf {
        let item_path = repo.join(path);
        std::fs::write(&item_path, contents).unwrap();
        let mut ctx = context::build_create_or_load(repo, Some(store)).unwrap();
        crate::api::item::add_file(&mut ctx, &item_path, false).unwrap();
        ctx.store_path_for(path)
    }

    #[test]
    fn healthy_associated_symlink_repository_has_no_issues() {
        if !common::require_symlink_support() {
            return;
        }
        let repo = common::init_git_repo();
        let store = TempDir::new().unwrap();
        add_item(repo.path(), store.path(), "secret.txt", "secret");

        let report = verify(store.path()).unwrap();

        assert!(
            report.is_healthy(),
            "unexpected issues: {:#?}",
            report.issues
        );
        assert_eq!(report.canonical_items_checked, 1);
    }

    #[test]
    fn associated_local_repository_reports_git_and_exclude_issues() {
        if !common::require_symlink_support() {
            return;
        }
        let repo = common::init_git_repo();
        let store = TempDir::new().unwrap();
        add_item(repo.path(), store.path(), "secret.txt", "secret");
        std::fs::write(repo.path().join(".git/info/exclude"), "").unwrap();
        common::run_git(repo.path(), &["add", "secret.txt"]);

        let report = verify(store.path()).unwrap();

        for code in [
            StatusIssueCode::MissingExclude,
            StatusIssueCode::TrackedByGit,
        ] {
            assert!(report.issues.iter().any(|issue| {
                issue.code == StoreVerifyIssueCode::LocalMaterialization(code)
                    && issue.severity == StoreVerifySeverity::Error
            }));
        }
    }

    #[test]
    fn unavailable_local_repository_is_a_warning_after_canonical_check() {
        if !common::require_symlink_support() {
            return;
        }
        let repo = common::init_git_repo();
        let store = TempDir::new().unwrap();
        add_item(repo.path(), store.path(), "secret.txt", "secret");

        let mut index = index::load(store.path()).unwrap();
        let repo_id = index.iter().next().unwrap().0.to_string();
        let mut entry = index.get(&repo_id).unwrap().clone();
        entry.root = Some(store.path().join("missing-local-repository"));
        index.upsert(&repo_id, entry);
        index::save(store.path(), &index).unwrap();

        let report = verify(store.path()).unwrap();

        assert_eq!(report.canonical_items_checked, 1);
        assert!(report.issues.iter().any(|issue| {
            issue.code == StoreVerifyIssueCode::LocalRepositoryUnavailable
                && issue.severity == StoreVerifySeverity::Warning
        }));
        assert!(!report.has_errors());
    }

    #[test]
    fn missing_canonical_item_is_reported_without_an_index() {
        let store = TempDir::new().unwrap();
        let repo_store = store.path().join("repos/project");
        let mut manifest = crate::store::manifest::Manifest::new("repo-1", "2026-07-14T00:00:00Z");
        manifest.add(crate::store::manifest::Item {
            item_id: "item-1".into(),
            origin_repo_id: "repo-1".into(),
            path: "secret.txt".into(),
            store_path: "items/secret.txt".into(),
            ownership_state: crate::store::manifest::OwnershipState::Attached,
            created_at: "2026-07-14T00:00:00Z".into(),
            updated_at: "2026-07-14T00:00:00Z".into(),
        });
        manifest::save(&repo_store, &manifest).unwrap();

        let report = verify(store.path()).unwrap();

        assert!(report.issues.iter().any(|issue| {
            issue.code == StoreVerifyIssueCode::CanonicalStoreMissing
                && issue.severity == StoreVerifySeverity::Error
        }));
    }
}
