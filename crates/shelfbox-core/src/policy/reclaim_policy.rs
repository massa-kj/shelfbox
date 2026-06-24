use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    domain::{manifest::Manifest, ownership::OwnershipState},
    error::{AppError, Result},
    plan::repo_reclaim::{CandidateState, ReclaimCandidate},
};

pub(crate) struct ReclaimCandidateFacts<'a> {
    pub repo_store_dir: String,
    pub manifest: Manifest,
    pub current_repo_name: &'a str,
    pub current_remote_hint: Option<&'a str>,
    pub has_manifest_warning: bool,
    pub items_dir_exists: bool,
}

pub(crate) fn score_candidate(facts: ReclaimCandidateFacts<'_>) -> ReclaimCandidate {
    let ReclaimCandidateFacts {
        repo_store_dir,
        manifest,
        current_repo_name,
        current_remote_hint,
        has_manifest_warning,
        items_dir_exists,
    } = facts;

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

    if !items_dir_exists {
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

pub(crate) fn candidate_state(manifest: &Manifest) -> CandidateState {
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

/// Returns `Err` if the current repo already has managed items.
pub(crate) fn check_reclaim_precondition(current_manifest: Option<&Manifest>) -> Result<()> {
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
    use crate::domain::manifest::Item;

    fn item(repo_id: &str, id: &str, state: OwnershipState) -> Item {
        let path = format!("{id}.env");
        Item {
            item_id: id.into(),
            origin_repo_id: repo_id.into(),
            path: path.clone(),
            store_path: format!("items/{path}"),
            ownership_state: state,
            created_at: "2026-04-29T00:00:00Z".into(),
            updated_at: "2026-04-29T00:00:00Z".into(),
        }
    }

    #[test]
    fn candidate_score_uses_hints_without_granting_identity() {
        let mut manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        manifest.add_remote_hint("github.com/example/app");
        manifest.add_repo_name_hint("/work/app");
        manifest.add(item("repo-1", "item-1", OwnershipState::Attached));

        let candidate = score_candidate(ReclaimCandidateFacts {
            repo_store_dir: "app".into(),
            manifest,
            current_repo_name: "app",
            current_remote_hint: Some("github.com/example/app"),
            has_manifest_warning: false,
            items_dir_exists: true,
        });

        assert_eq!(candidate.score, 230);
        assert_eq!(
            candidate.reasons,
            vec![
                "remote matched",
                "store dir matched",
                "repo name hint matched",
                "has managed items"
            ]
        );
        assert_eq!(candidate.state, CandidateState::AttachedElsewhere);
    }

    #[test]
    fn warning_and_missing_items_dir_reduce_score() {
        let manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");

        let candidate = score_candidate(ReclaimCandidateFacts {
            repo_store_dir: "other".into(),
            manifest,
            current_repo_name: "app",
            current_remote_hint: None,
            has_manifest_warning: true,
            items_dir_exists: false,
        });

        assert_eq!(candidate.score, -150);
        assert_eq!(
            candidate.reasons,
            vec!["manifest warning", "items directory missing"]
        );
    }

    #[test]
    fn candidate_state_prioritizes_unreachable_over_detached() {
        let mut manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        manifest.add(item("repo-1", "item-1", OwnershipState::Detached));
        assert_eq!(candidate_state(&manifest), CandidateState::Detached);

        manifest.add(item("repo-1", "item-2", OwnershipState::Unreachable));
        assert_eq!(candidate_state(&manifest), CandidateState::Unreachable);
    }

    #[test]
    fn reclaim_precondition_fails_when_current_repo_has_items() {
        let mut manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        manifest.add(item("repo-1", "item-1", OwnershipState::Attached));

        let err = check_reclaim_precondition(Some(&manifest)).unwrap_err();

        assert!(err.to_string().contains("already has managed items"));
    }

    #[test]
    fn reclaim_precondition_passes_for_uninitialized_or_empty_repo() {
        let manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");

        check_reclaim_precondition(None).unwrap();
        check_reclaim_precondition(Some(&manifest)).unwrap();
    }
}
