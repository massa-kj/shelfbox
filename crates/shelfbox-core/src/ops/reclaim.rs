use std::{
    collections::BTreeSet,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    context::{self, CurrentGitContext},
    error::{AppError, Result},
    store::{
        index::{self, Index, RepoEntry},
        manifest::{Manifest, OwnershipState},
        scanner::{self, ScanError},
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclaimCandidate {
    pub repo_store_dir: String,
    pub repo_id: String,
    pub score: i32,
    pub reasons: Vec<String>,
    pub item_count: usize,
    pub state: CandidateState,
    pub remote_hints: Vec<String>,
    pub last_attached_at: Option<String>,
    pub repo_name_hints: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateState {
    Unreachable,
    Detached,
    AttachedElsewhere,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclaimOutcome {
    pub repo_id: String,
    pub repo_store_dir: String,
}

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
        candidates.push(score_candidate(
            store_root,
            scanned.repo_store_dir,
            scanned.manifest,
            current_repo_name,
            current_remote_hint,
            has_manifest_warning,
        ));
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
    if current_manifest
        .map(|manifest| !manifest.items.is_empty())
        .unwrap_or(false)
    {
        return Err(AppError::Internal(
            "cannot reclaim into a repository that already has managed items".into(),
        ));
    }
    Ok(())
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

    let mut manifest = target.manifest;
    let repo_store_dir = target.repo_store_dir;
    let repo_store = store_root.join("repos").join(&repo_store_dir);
    let now = context::now_iso8601();

    if let Some(name) = current.repo_root.file_name().and_then(|name| name.to_str()) {
        manifest.add_repo_name_hint(name);
    }
    if let Some(remote_hint) = &current.remote_hint {
        manifest.add_remote_hint(remote_hint);
    }
    manifest.touch_attached_at(now.clone());

    let mut idx = index::load(store_root)?;
    idx.upsert(
        target_repo_id,
        RepoEntry {
            repo_store_dir: repo_store_dir.clone(),
            root: Some(current.repo_root.clone()),
            git_dir: Some(current.git_dir.clone()),
            git_common_dir: Some(current.git_common_dir.clone()),
            last_seen_at: now,
        },
    );

    index::save(store_root, &idx)?;
    crate::store::manifest::save(&repo_store, &manifest)?;

    Ok(ReclaimOutcome {
        repo_id: target_repo_id.to_string(),
        repo_store_dir,
    })
}

fn score_candidate(
    store_root: &Path,
    repo_store_dir: String,
    manifest: Manifest,
    current_repo_name: &str,
    current_remote_hint: Option<&str>,
    has_manifest_warning: bool,
) -> ReclaimCandidate {
    let mut score = 0;
    let mut reasons = Vec::new();

    if let Some(remote_hint) = current_remote_hint {
        if manifest
            .identity_hints
            .remote_hints
            .iter()
            .any(|hint| hint == remote_hint)
        {
            score += 100;
            reasons.push("remote matched".into());
        }
    }

    if !current_repo_name.is_empty() && repo_store_dir == current_repo_name {
        score += 60;
        reasons.push("store dir matched".into());
    }

    if !current_repo_name.is_empty()
        && manifest
            .identity_hints
            .repo_name_hints
            .iter()
            .any(|hint| hint_basename(hint) == current_repo_name)
    {
        score += 40;
        reasons.push("repo name hint matched".into());
    }

    let item_count = manifest.items.len();
    if item_count > 0 {
        score += 30;
        reasons.push("has managed items".into());
    }

    if manifest
        .identity_hints
        .last_attached_at
        .as_deref()
        .is_some_and(is_recent_last_attached_at)
    {
        score += 20;
        reasons.push("recently attached".into());
    }

    if has_manifest_warning {
        score -= 50;
        reasons.push("manifest warning".into());
    }

    if !store_root
        .join("repos")
        .join(&repo_store_dir)
        .join("items")
        .is_dir()
    {
        score -= 100;
        reasons.push("items directory missing".into());
    }

    let state = candidate_state(&manifest);
    let remote_hints = manifest.identity_hints.remote_hints.clone();
    let last_attached_at = manifest.identity_hints.last_attached_at.clone();
    let repo_name_hints = manifest.identity_hints.repo_name_hints.clone();

    ReclaimCandidate {
        repo_store_dir,
        repo_id: manifest.repo_id,
        score,
        reasons,
        item_count,
        state,
        remote_hints,
        last_attached_at,
        repo_name_hints,
    }
}

fn candidate_state(manifest: &Manifest) -> CandidateState {
    if manifest
        .items
        .iter()
        .any(|item| item.ownership_state == OwnershipState::Unreachable)
    {
        CandidateState::Unreachable
    } else if manifest
        .items
        .iter()
        .any(|item| item.ownership_state == OwnershipState::Detached)
    {
        CandidateState::Detached
    } else {
        CandidateState::AttachedElsewhere
    }
}

fn hint_basename(hint: &str) -> &str {
    hint.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(hint)
}

fn is_recent_last_attached_at(value: &str) -> bool {
    let Some(attached_at) = parse_iso8601_utc(value) else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let age = now - attached_at;
    (0..=90 * 24 * 60 * 60).contains(&age)
}

fn parse_iso8601_utc(value: &str) -> Option<i64> {
    let value = value.strip_suffix('Z').unwrap_or(value);
    let (date, time) = value.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }

    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;
    if time_parts.next().is_some()
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }

    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month_prime + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::context::CurrentGitContext;
    use crate::store::{
        index::{self, RepoEntry},
        manifest::{self, Item},
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
