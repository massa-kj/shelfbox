use std::{collections::BTreeSet, path::Path};

use crate::{
    context::{self, CurrentGitContext},
    error::{AppError, Result},
    policy::reclaim_policy::{self, ReclaimCandidateFacts},
    store::{
        index::{self, Index, RepoEntry},
        manifest::Manifest,
        scanner::{self, ScanError},
    },
};

pub use crate::plan::repo_reclaim::{
    CandidateState, ReclaimCandidate, ReclaimOutcome, ReclaimPlan,
};

pub fn build_candidates(
    store_root: &Path,
    current_repo_root: &Path,
    current_remote_hint: Option<&str>,
    index: &Index,
) -> Result<Vec<ReclaimCandidate>> {
    let scan = scanner::scan(store_root)?;
    let mut warning_dirs = BTreeSet::new();

    for error in &scan.errors {
        match error {
            ScanError::ReadFailed { dir, .. } | ScanError::ParseFailed { dir, .. } => {
                warning_dirs.insert(dir.clone());
            }
            ScanError::DuplicateRepoId { repo_id, dirs } => {
                return Err(AppError::Internal(format!(
                    "cannot build reclaim candidates: duplicate repo_id '{repo_id}' found in {}",
                    dirs.join(", ")
                )));
            }
            ScanError::DuplicateItemId { item_id, repo_ids } => {
                return Err(AppError::Internal(format!(
                    "cannot build reclaim candidates: duplicate item_id '{item_id}' found in repo_id(s) {}",
                    repo_ids.join(", ")
                )));
            }
        }
    }

    let current_repo_name = current_repo_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let mut candidates = Vec::new();

    for scanned in scan.entries {
        if index
            .get(&scanned.manifest.repo_id)
            .and_then(|entry| entry.root.as_deref())
            == Some(current_repo_root)
        {
            continue;
        }

        let has_manifest_warning = warning_dirs.contains(&scanned.repo_store_dir);
        let repo_store_dir = scanned.repo_store_dir;
        let items_dir_exists = store_root
            .join("repos")
            .join(&repo_store_dir)
            .join("items")
            .is_dir();
        candidates.push(reclaim_policy::score_candidate(ReclaimCandidateFacts {
            repo_store_dir,
            manifest: scanned.manifest,
            current_repo_name,
            current_remote_hint,
            has_manifest_warning,
            items_dir_exists,
        }));
    }

    candidates.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.repo_store_dir.cmp(&b.repo_store_dir))
            .then_with(|| a.repo_id.cmp(&b.repo_id))
    });
    Ok(candidates)
}

/// Returns `Err` if the current repo already has managed items.
pub fn check_reclaim_precondition(current_manifest: Option<&Manifest>) -> Result<()> {
    reclaim_policy::check_reclaim_precondition(current_manifest)
}

/// Associates the current Git checkout with an existing `RepoId`.
///
/// This updates only local index metadata and target manifest identity hints.
/// It does not move items, mutate item ownership, repair symlinks, or rewrite
/// ignore/exclude files.
pub fn execute_reclaim(
    store_root: &Path,
    current: &CurrentGitContext,
    target_repo_id: &str,
) -> Result<ReclaimOutcome> {
    let plan = plan_reclaim(store_root, current, target_repo_id)?;
    execute_reclaim_plan(store_root, &plan)
}

/// Builds the explicit metadata changes needed to associate this checkout with
/// an existing `RepoId`.
pub fn plan_reclaim(
    store_root: &Path,
    current: &CurrentGitContext,
    target_repo_id: &str,
) -> Result<ReclaimPlan> {
    let scan = scanner::scan(store_root)?;
    if let Some(error) = scan.errors.first() {
        match error {
            ScanError::ReadFailed { dir, source } => {
                return Err(AppError::Internal(format!(
                    "cannot reclaim: failed to read repos/{dir}/manifest.json: {source}"
                )));
            }
            ScanError::ParseFailed { dir, source } => {
                return Err(AppError::Internal(format!(
                    "cannot reclaim: failed to parse repos/{dir}/manifest.json: {source}"
                )));
            }
            ScanError::DuplicateRepoId { repo_id, dirs } => {
                return Err(AppError::Internal(format!(
                    "cannot reclaim: duplicate repo_id '{repo_id}' found in {}",
                    dirs.join(", ")
                )));
            }
            ScanError::DuplicateItemId { item_id, repo_ids } => {
                return Err(AppError::Internal(format!(
                    "cannot reclaim: duplicate item_id '{item_id}' found in repo_id(s) {}",
                    repo_ids.join(", ")
                )));
            }
        }
    }

    let target = scan
        .entries
        .into_iter()
        .find(|entry| entry.manifest.repo_id == target_repo_id)
        .ok_or_else(|| {
            AppError::Internal(format!(
                "cannot reclaim: target repo_id '{target_repo_id}' not found"
            ))
        })?;

    let repo_store_dir = target.repo_store_dir;
    let now = context::now_iso8601();
    let idx = index::load(store_root)?;
    let mut removed_association_ids: Vec<String> = idx
        .iter()
        .filter(|(repo_id, entry)| {
            *repo_id != target_repo_id
                && (entry.root.as_deref() == Some(current.repo_root.as_path())
                    || entry.git_common_dir.as_deref() == Some(current.git_common_dir.as_path()))
        })
        .map(|(repo_id, _)| repo_id.to_string())
        .collect();
    removed_association_ids.sort();

    Ok(ReclaimPlan {
        repo_id: target_repo_id.to_string(),
        repo_store_dir: repo_store_dir.clone(),
        repo_store: store_root.join("repos").join(&repo_store_dir),
        current_root: current.repo_root.clone(),
        current_git_dir: current.git_dir.clone(),
        current_git_common_dir: current.git_common_dir.clone(),
        removed_association_ids,
        repo_name_hint: current
            .repo_root
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned),
        remote_hint: current.remote_hint.clone(),
        attached_at: now.clone(),
        last_seen_at: now,
    })
}

/// Executes a reclaim plan by updating only the target manifest identity hints
/// and local index associations.
pub fn execute_reclaim_plan(store_root: &Path, plan: &ReclaimPlan) -> Result<ReclaimOutcome> {
    let repo_store = store_root.join("repos").join(&plan.repo_store_dir);
    if plan.repo_store != repo_store {
        return Err(AppError::Internal(format!(
            "cannot reclaim: plan repo_store '{}' does not match expected '{}'",
            plan.repo_store.display(),
            repo_store.display()
        )));
    }

    let original_manifest = crate::store::manifest::load(&repo_store)?;
    if original_manifest.repo_id != plan.repo_id {
        return Err(AppError::Internal(format!(
            "cannot reclaim: plan repo_id '{}' does not match manifest repo_id '{}'",
            plan.repo_id, original_manifest.repo_id
        )));
    }

    let mut manifest = original_manifest.clone();
    if let Some(name) = &plan.repo_name_hint {
        manifest.add_repo_name_hint(name);
    }
    if let Some(remote_hint) = &plan.remote_hint {
        manifest.add_remote_hint(remote_hint);
    }
    manifest.touch_attached_at(plan.attached_at.clone());

    let mut idx = index::load(store_root)?;
    let mut current_association_ids: Vec<String> = idx
        .iter()
        .filter(|(repo_id, entry)| {
            *repo_id != plan.repo_id
                && (entry.root.as_deref() == Some(plan.current_root.as_path())
                    || entry.git_common_dir.as_deref()
                        == Some(plan.current_git_common_dir.as_path()))
        })
        .map(|(repo_id, _)| repo_id.to_string())
        .collect();
    current_association_ids.sort();
    if current_association_ids != plan.removed_association_ids {
        return Err(AppError::Internal(
            "cannot reclaim: store index changed since reclaim plan was built".into(),
        ));
    }

    for repo_id in &plan.removed_association_ids {
        idx.remove(repo_id);
    }
    idx.upsert(
        &plan.repo_id,
        RepoEntry {
            repo_store_dir: plan.repo_store_dir.clone(),
            root: Some(plan.current_root.clone()),
            git_dir: Some(plan.current_git_dir.clone()),
            git_common_dir: Some(plan.current_git_common_dir.clone()),
            last_seen_at: plan.last_seen_at.clone(),
        },
    );

    crate::store::manifest::save(&repo_store, &manifest)?;
    if let Err(err) = index::save(store_root, &idx) {
        let _ = crate::store::manifest::save(&repo_store, &original_manifest);
        return Err(err);
    }

    Ok(ReclaimOutcome {
        repo_id: plan.repo_id.clone(),
        repo_store_dir: plan.repo_store_dir.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::context::CurrentGitContext;
    use crate::store::{
        index::{self, RepoEntry},
        manifest::{self, Item, OwnershipState},
    };
    use tempfile::TempDir;

    fn item(repo_id: &str, id: &str, state: OwnershipState) -> Item {
        Item {
            item_id: id.into(),
            origin_repo_id: repo_id.into(),
            path: ".env".into(),
            store_path: "items/.env".into(),
            ownership_state: state,
            created_at: "2026-04-29T00:00:00Z".into(),
            updated_at: "2026-04-29T00:00:00Z".into(),
        }
    }

    fn write_manifest(store_root: &Path, dir: &str, manifest: &Manifest, create_items: bool) {
        let repo_store = store_root.join("repos").join(dir);
        manifest::save(&repo_store, manifest).unwrap();
        if create_items {
            std::fs::create_dir_all(repo_store.join("items")).unwrap();
        }
    }

    fn write_raw_manifest(store_root: &Path, dir: &str, contents: &str) {
        let repo_store = store_root.join("repos").join(dir);
        std::fs::create_dir_all(&repo_store).unwrap();
        std::fs::write(manifest::manifest_path(&repo_store), contents).unwrap();
    }

    fn current_repo_root(name: &str) -> (TempDir, PathBuf) {
        let base = TempDir::new().unwrap();
        let root = base.path().join(name);
        std::fs::create_dir(&root).unwrap();
        (base, root)
    }

    fn current_context(root: PathBuf) -> CurrentGitContext {
        CurrentGitContext {
            git_dir: root.join(".git"),
            git_common_dir: root.join(".git"),
            repo_root: root,
            remote_hint: Some("github.com/example/current".into()),
        }
    }

    #[test]
    fn remote_match_scores_100() {
        let store = TempDir::new().unwrap();
        let (_base, current) = current_repo_root("current");
        let mut manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        manifest.add_remote_hint("github.com/example/current");
        write_manifest(store.path(), "other", &manifest, true);

        let candidates = build_candidates(
            store.path(),
            &current,
            Some("github.com/example/current"),
            &Index::new(),
        )
        .unwrap();

        assert_eq!(candidates[0].score, 100);
        assert_eq!(candidates[0].reasons, vec!["remote matched"]);
    }

    #[test]
    fn dir_name_match_scores_60() {
        let store = TempDir::new().unwrap();
        let (_base, current) = current_repo_root("current");
        let manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        write_manifest(store.path(), "current", &manifest, true);

        let candidates = build_candidates(store.path(), &current, None, &Index::new()).unwrap();

        assert_eq!(candidates[0].score, 60);
        assert_eq!(candidates[0].reasons, vec!["store dir matched"]);
    }

    #[test]
    fn no_match_scores_zero_base() {
        let store = TempDir::new().unwrap();
        let (_base, current) = current_repo_root("current");
        let manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        write_manifest(store.path(), "other", &manifest, true);

        let candidates = build_candidates(store.path(), &current, None, &Index::new()).unwrap();

        assert_eq!(candidates[0].score, 0);
        assert!(candidates[0].reasons.is_empty());
    }

    #[test]
    fn missing_items_dir_scores_minus_100() {
        let store = TempDir::new().unwrap();
        let (_base, current) = current_repo_root("current");
        let manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        write_manifest(store.path(), "other", &manifest, false);

        let candidates = build_candidates(store.path(), &current, None, &Index::new()).unwrap();

        assert_eq!(candidates[0].score, -100);
        assert_eq!(candidates[0].reasons, vec!["items directory missing"]);
    }

    #[test]
    fn result_is_sorted_by_score_descending() {
        let store = TempDir::new().unwrap();
        let (_base, current) = current_repo_root("current");

        let low = Manifest::new("repo-low", "2026-04-29T00:00:00Z");
        write_manifest(store.path(), "low", &low, true);

        let mut high = Manifest::new("repo-high", "2026-04-29T00:00:00Z");
        high.add_remote_hint("github.com/example/current");
        write_manifest(store.path(), "high", &high, true);

        let candidates = build_candidates(
            store.path(),
            &current,
            Some("github.com/example/current"),
            &Index::new(),
        )
        .unwrap();

        assert_eq!(candidates[0].repo_id, "repo-high");
        assert_eq!(candidates[1].repo_id, "repo-low");
    }

    #[test]
    fn build_candidates_does_not_mutate_files() {
        let store = TempDir::new().unwrap();
        let (_base, current) = current_repo_root("current");
        let manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        write_manifest(store.path(), "other", &manifest, true);
        let manifest_path = store.path().join("repos/other/manifest.json");
        let before = std::fs::read_to_string(&manifest_path).unwrap();

        let _ = build_candidates(store.path(), &current, None, &Index::new()).unwrap();

        let after = std::fs::read_to_string(&manifest_path).unwrap();
        assert_eq!(after, before);
        assert!(!store.path().join("index.json").exists());
    }

    #[test]
    fn precondition_fails_when_current_repo_has_items() {
        let mut manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        manifest.add(item("repo-1", "item-1", OwnershipState::Attached));

        let err = check_reclaim_precondition(Some(&manifest)).unwrap_err();

        assert!(err.to_string().contains("already has managed items"));
    }

    #[test]
    fn precondition_passes_for_uninitialized_or_empty_repo() {
        let manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");

        check_reclaim_precondition(None).unwrap();
        check_reclaim_precondition(Some(&manifest)).unwrap();
    }

    #[test]
    fn plan_reclaim_reports_explicit_mutations_without_writing() {
        let store = TempDir::new().unwrap();
        let (_base, root) = current_repo_root("current");
        let current = current_context(root.clone());
        let manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        write_manifest(store.path(), "target", &manifest, true);

        let mut idx = Index::new();
        idx.upsert(
            "old-current",
            RepoEntry {
                root: Some(root.clone()),
                git_dir: Some(root.join(".git")),
                git_common_dir: Some(root.join(".git")),
                repo_store_dir: "old-current".into(),
                last_seen_at: "2026-04-29T00:00:00Z".into(),
            },
        );
        idx.upsert(
            "other",
            RepoEntry {
                root: Some(root.join("other")),
                git_dir: None,
                git_common_dir: None,
                repo_store_dir: "other".into(),
                last_seen_at: "2026-04-29T00:00:00Z".into(),
            },
        );
        index::save(store.path(), &idx).unwrap();

        let manifest_path = store.path().join("repos/target/manifest.json");
        let index_path = index::index_path(store.path());
        let manifest_before = std::fs::read_to_string(&manifest_path).unwrap();
        let index_before = std::fs::read_to_string(&index_path).unwrap();

        let plan = plan_reclaim(store.path(), &current, "repo-1").unwrap();

        assert_eq!(plan.repo_id, "repo-1");
        assert_eq!(plan.repo_store_dir, "target");
        assert_eq!(plan.repo_store, store.path().join("repos/target"));
        assert_eq!(plan.current_root, root);
        assert_eq!(plan.current_git_dir, plan.current_root.join(".git"));
        assert_eq!(plan.current_git_common_dir, plan.current_root.join(".git"));
        assert_eq!(plan.removed_association_ids, vec!["old-current"]);
        assert_eq!(plan.repo_name_hint.as_deref(), Some("current"));
        assert_eq!(
            plan.remote_hint.as_deref(),
            Some("github.com/example/current")
        );
        assert_eq!(plan.last_seen_at, plan.attached_at);

        assert_eq!(
            std::fs::read_to_string(manifest_path).unwrap(),
            manifest_before
        );
        assert_eq!(std::fs::read_to_string(index_path).unwrap(), index_before);
    }

    #[test]
    fn execute_reclaim_plan_rejects_stale_index_without_writing() {
        let store = TempDir::new().unwrap();
        let (_base, root) = current_repo_root("current");
        let current = current_context(root.clone());
        let manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        write_manifest(store.path(), "target", &manifest, true);

        let plan = plan_reclaim(store.path(), &current, "repo-1").unwrap();

        let mut idx = Index::new();
        idx.upsert(
            "late-current",
            RepoEntry {
                root: Some(root),
                git_dir: None,
                git_common_dir: Some(plan.current_git_common_dir.clone()),
                repo_store_dir: "late-current".into(),
                last_seen_at: "2026-04-29T00:00:00Z".into(),
            },
        );
        index::save(store.path(), &idx).unwrap();

        let manifest_path = store.path().join("repos/target/manifest.json");
        let index_path = index::index_path(store.path());
        let manifest_before = std::fs::read_to_string(&manifest_path).unwrap();
        let index_before = std::fs::read_to_string(&index_path).unwrap();

        let err = execute_reclaim_plan(store.path(), &plan).unwrap_err();

        assert!(err.to_string().contains("store index changed"));
        assert_eq!(
            std::fs::read_to_string(manifest_path).unwrap(),
            manifest_before
        );
        assert_eq!(std::fs::read_to_string(index_path).unwrap(), index_before);
    }

    #[test]
    fn execute_reclaim_updates_index_and_hints_without_item_mutation() {
        let store = TempDir::new().unwrap();
        let (_base, root) = current_repo_root("current");
        let current = current_context(root.clone());
        let mut manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        manifest.add(item("repo-1", "item-1", OwnershipState::Unreachable));
        write_manifest(store.path(), "target", &manifest, true);

        let outcome = execute_reclaim(store.path(), &current, "repo-1").unwrap();

        assert_eq!(outcome.repo_id, "repo-1");
        assert_eq!(outcome.repo_store_dir, "target");

        let idx = index::load(store.path()).unwrap();
        let entry = idx.get("repo-1").unwrap();
        assert_eq!(entry.repo_store_dir, "target");
        assert_eq!(entry.root.as_deref(), Some(root.as_path()));
        assert_eq!(entry.git_dir.as_deref(), Some(root.join(".git").as_path()));
        assert_eq!(
            entry.git_common_dir.as_deref(),
            Some(root.join(".git").as_path())
        );

        let loaded = manifest::load(&store.path().join("repos/target")).unwrap();
        assert_eq!(loaded.items, manifest.items, "items must not be mutated");
        assert_eq!(
            loaded.identity_hints.remote_hints,
            vec!["github.com/example/current"]
        );
        assert_eq!(
            loaded
                .identity_hints
                .repo_name_hints
                .first()
                .map(String::as_str),
            Some("current")
        );
        assert!(loaded.identity_hints.last_attached_at.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn execute_reclaim_rolls_back_manifest_when_index_save_fails() {
        use std::os::unix::fs::PermissionsExt;

        let store = TempDir::new().unwrap();
        let (_base, root) = current_repo_root("current");
        let current = current_context(root);
        let mut manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        manifest.add(item("repo-1", "item-1", OwnershipState::Unreachable));
        write_manifest(store.path(), "target", &manifest, true);

        let original_manifest =
            std::fs::read_to_string(store.path().join("repos/target/manifest.json")).unwrap();
        let original_perms = std::fs::metadata(store.path()).unwrap().permissions();
        let mut readonly_perms = original_perms.clone();
        readonly_perms.set_mode(0o500);
        std::fs::set_permissions(store.path(), readonly_perms).unwrap();

        let result = execute_reclaim(store.path(), &current, "repo-1");

        std::fs::set_permissions(store.path(), original_perms).unwrap();
        assert!(result.is_err());
        assert!(!index::index_path(store.path()).exists());
        assert_eq!(
            std::fs::read_to_string(store.path().join("repos/target/manifest.json")).unwrap(),
            original_manifest
        );
    }

    #[test]
    fn execute_reclaim_from_uninitialized_current_repo_creates_no_throwaway_repo_id() {
        let store = TempDir::new().unwrap();
        let (_base, root) = current_repo_root("current");
        let current = current_context(root);
        write_manifest(
            store.path(),
            "target",
            &Manifest::new("repo-1", "2026-04-29T00:00:00Z"),
            true,
        );

        execute_reclaim(store.path(), &current, "repo-1").unwrap();

        let idx = index::load(store.path()).unwrap();
        assert_eq!(idx.iter().count(), 1);
        assert!(idx.get("repo-1").is_some());
        assert!(!store.path().join("repos/current").exists());
    }

    #[test]
    fn execute_reclaim_unknown_repo_id_errors_without_writing_index() {
        let store = TempDir::new().unwrap();
        let (_base, root) = current_repo_root("current");
        let current = current_context(root);
        write_manifest(
            store.path(),
            "target",
            &Manifest::new("repo-1", "2026-04-29T00:00:00Z"),
            true,
        );

        let err = execute_reclaim(store.path(), &current, "missing").unwrap_err();

        assert!(err
            .to_string()
            .contains("target repo_id 'missing' not found"));
        assert!(!index::index_path(store.path()).exists());
    }

    #[test]
    fn execute_reclaim_corrupted_manifest_errors_without_writing_index() {
        let store = TempDir::new().unwrap();
        let (_base, root) = current_repo_root("current");
        let current = current_context(root);
        write_manifest(
            store.path(),
            "target",
            &Manifest::new("repo-1", "2026-04-29T00:00:00Z"),
            true,
        );
        write_raw_manifest(store.path(), "bad", "{not json");

        let err = execute_reclaim(store.path(), &current, "repo-1").unwrap_err();

        assert!(err.to_string().contains("failed to parse"));
        assert!(!index::index_path(store.path()).exists());
    }

    #[test]
    fn execute_reclaim_duplicate_repo_id_errors_without_writing_index() {
        let store = TempDir::new().unwrap();
        let (_base, root) = current_repo_root("current");
        let current = current_context(root);
        write_manifest(
            store.path(),
            "a",
            &Manifest::new("repo-1", "2026-04-29T00:00:00Z"),
            true,
        );
        write_manifest(
            store.path(),
            "b",
            &Manifest::new("repo-1", "2026-04-29T00:00:00Z"),
            true,
        );

        let err = execute_reclaim(store.path(), &current, "repo-1").unwrap_err();

        assert!(err.to_string().contains("duplicate repo_id"));
        assert!(!index::index_path(store.path()).exists());
    }

    #[test]
    fn duplicate_repo_id_is_hard_error() {
        let store = TempDir::new().unwrap();
        let (_base, current) = current_repo_root("current");
        write_manifest(
            store.path(),
            "a",
            &Manifest::new("repo-1", "2026-04-29T00:00:00Z"),
            true,
        );
        write_manifest(
            store.path(),
            "b",
            &Manifest::new("repo-1", "2026-04-29T00:00:00Z"),
            true,
        );

        let err = build_candidates(store.path(), &current, None, &Index::new()).unwrap_err();

        assert!(err.to_string().contains("duplicate repo_id"));
        assert!(!store.path().join("index.json").exists());
    }

    #[test]
    fn skips_repo_already_associated_with_current_root() {
        let store = TempDir::new().unwrap();
        let (_base, current) = current_repo_root("current");
        let manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        write_manifest(store.path(), "current", &manifest, true);

        let mut index = Index::new();
        index.upsert(
            "repo-1",
            RepoEntry {
                root: Some(current.clone()),
                git_dir: None,
                git_common_dir: None,
                repo_store_dir: "current".into(),
                last_seen_at: "2026-04-29T00:00:00Z".into(),
            },
        );

        let candidates = build_candidates(store.path(), &current, None, &index).unwrap();

        assert!(candidates.is_empty());
    }
}
