use std::{collections::BTreeSet, path::PathBuf};

use crate::{
    domain::{manifest::Manifest, ownership::OwnershipState},
    plan::repo_repair::RepoRepairSymlinkAction,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SymlinkRepairDecision {
    AlreadyHealthy,
    Recreate,
    RefuseRegularFile,
    RefuseUnexpectedTarget { actual_target: PathBuf },
}

/// Policy-facing materialization facts. Filesystem identity, link targets,
/// and paths remain in the adapter/operation layers; this enum is deliberately
/// just enough to choose a non-destructive repair outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepairMaterializationState {
    Missing,
    ManagedSymlink,
    UnmanagedSymlink,
    RegularEqual,
    RegularDiverged,
    RegularUnknown,
    StoreMissing,
    Unsafe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MaterializationRepairDecision {
    AlreadyHealthy,
    Create,
    CopyDiverged,
    StoreMissing,
    RefuseRegular,
    DelegateSymlinkPolicy,
}

/// Decides whether a materialization can be repaired without overwriting
/// existing regular content. A configured strategy selects only the `Create`
/// mechanism; it never changes an existing regular Copy into another form.
pub(crate) fn decide_materialization_repair(
    state: RepairMaterializationState,
) -> MaterializationRepairDecision {
    match state {
        RepairMaterializationState::Missing => MaterializationRepairDecision::Create,
        RepairMaterializationState::ManagedSymlink | RepairMaterializationState::RegularEqual => {
            MaterializationRepairDecision::AlreadyHealthy
        }
        RepairMaterializationState::RegularDiverged => MaterializationRepairDecision::CopyDiverged,
        RepairMaterializationState::StoreMissing => MaterializationRepairDecision::StoreMissing,
        RepairMaterializationState::UnmanagedSymlink => {
            MaterializationRepairDecision::DelegateSymlinkPolicy
        }
        RepairMaterializationState::RegularUnknown | RepairMaterializationState::Unsafe => {
            MaterializationRepairDecision::RefuseRegular
        }
    }
}

pub(crate) fn decide_symlink_repair(
    is_managed_link: bool,
    path_exists: bool,
    is_link: bool,
    actual_target: Option<PathBuf>,
    force: bool,
) -> SymlinkRepairDecision {
    if is_managed_link {
        return SymlinkRepairDecision::AlreadyHealthy;
    }

    if !is_link && path_exists {
        return SymlinkRepairDecision::RefuseRegularFile;
    }

    if !force {
        if let Some(actual_target) = actual_target {
            return SymlinkRepairDecision::RefuseUnexpectedTarget { actual_target };
        }
    }

    SymlinkRepairDecision::Recreate
}

#[cfg(test)]
pub(crate) fn attached_item_paths(manifest: &Manifest) -> Vec<String> {
    manifest
        .items
        .iter()
        .filter(|item| item.ownership_state == OwnershipState::Attached)
        .map(|item| item.path.clone())
        .collect()
}

/// Computes the exact managed exclude set for repo repair without changing the
/// manifest. Attached items are always integrated. Detached items are only
/// newly included when their origin repository still has an index entry; when
/// that proof is absent, a pre-existing managed exclude is preserved but no
/// new one is introduced. Unreachable and orphaned items are deliberately
/// excluded from this set.
pub(crate) fn repo_repair_exclude_paths(
    manifest: &Manifest,
    existing: &BTreeSet<String>,
    has_repo_entry: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut desired = BTreeSet::new();
    for item in &manifest.items {
        match item.ownership_state {
            OwnershipState::Attached => {
                desired.insert(item.path.clone());
            }
            OwnershipState::Detached => {
                if has_repo_entry(&item.origin_repo_id) || existing.contains(&item.path) {
                    desired.insert(item.path.clone());
                }
            }
            OwnershipState::Unreachable | OwnershipState::Orphaned => {}
        }
    }
    desired.into_iter().collect()
}

pub(crate) fn action_blocks_identity_hint_update(action: &RepoRepairSymlinkAction) -> bool {
    matches!(
        action,
        RepoRepairSymlinkAction::StoreMissing { .. }
            | RepoRepairSymlinkAction::NotManaged { .. }
            | RepoRepairSymlinkAction::Failed { .. }
    )
}

pub(crate) fn identity_hints_update_allowed(symlink_actions: &[RepoRepairSymlinkAction]) -> bool {
    !symlink_actions
        .iter()
        .any(action_blocks_identity_hint_update)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::manifest::Item;

    fn item(path: &str, ownership_state: OwnershipState) -> Item {
        Item {
            item_id: format!("item-{path}"),
            origin_repo_id: "repo-1".into(),
            path: path.into(),
            store_path: format!("items/{path}"),
            ownership_state,
            created_at: "2026-04-29T00:00:00Z".into(),
            updated_at: "2026-04-29T00:00:00Z".into(),
        }
    }

    #[test]
    fn symlink_repair_decisions_require_force_for_wrong_targets() {
        assert_eq!(
            decide_symlink_repair(true, true, true, None, false),
            SymlinkRepairDecision::AlreadyHealthy
        );
        assert_eq!(
            decide_symlink_repair(false, true, false, None, false),
            SymlinkRepairDecision::RefuseRegularFile
        );
        assert_eq!(
            decide_symlink_repair(
                false,
                true,
                true,
                Some(PathBuf::from("/wrong/target")),
                false
            ),
            SymlinkRepairDecision::RefuseUnexpectedTarget {
                actual_target: PathBuf::from("/wrong/target")
            }
        );
        assert_eq!(
            decide_symlink_repair(
                false,
                true,
                true,
                Some(PathBuf::from("/wrong/target")),
                true
            ),
            SymlinkRepairDecision::Recreate
        );
    }

    #[test]
    fn repo_repair_operates_only_on_attached_items() {
        let mut manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        manifest.add(item("attached.env", OwnershipState::Attached));
        manifest.add(item("detached.env", OwnershipState::Detached));
        manifest.add(item("unreachable.env", OwnershipState::Unreachable));
        manifest.add(item("orphaned.env", OwnershipState::Orphaned));

        assert_eq!(attached_item_paths(&manifest), vec!["attached.env"]);
    }

    #[test]
    fn failed_or_missing_actions_block_identity_hint_updates() {
        let ok = vec![
            RepoRepairSymlinkAction::AlreadyHealthy {
                path: "a.env".into(),
            },
            RepoRepairSymlinkAction::Recreate {
                path: "b.env".into(),
                abs_path: PathBuf::from("/repo/b.env"),
                store_path: PathBuf::from("/store/items/b.env"),
            },
        ];
        assert!(identity_hints_update_allowed(&ok));

        let failed = vec![RepoRepairSymlinkAction::StoreMissing {
            path: "a.env".into(),
        }];
        assert!(!identity_hints_update_allowed(&failed));
    }

    #[test]
    fn materialization_repair_never_overwrites_regular_content() {
        assert_eq!(
            decide_materialization_repair(RepairMaterializationState::Missing),
            MaterializationRepairDecision::Create
        );
        assert_eq!(
            decide_materialization_repair(RepairMaterializationState::RegularEqual),
            MaterializationRepairDecision::AlreadyHealthy
        );
        assert_eq!(
            decide_materialization_repair(RepairMaterializationState::RegularDiverged),
            MaterializationRepairDecision::CopyDiverged
        );
        assert_eq!(
            decide_materialization_repair(RepairMaterializationState::RegularUnknown),
            MaterializationRepairDecision::RefuseRegular
        );
    }

    #[test]
    fn repo_repair_exclude_set_handles_detached_items_conservatively() {
        let mut manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        manifest.add(item("attached.env", OwnershipState::Attached));
        manifest.add(item("detached-known.env", OwnershipState::Detached));
        manifest.add(item("detached-missing.env", OwnershipState::Detached));
        manifest.items[2].origin_repo_id = "missing-repo".into();
        manifest.add(item("unreachable.env", OwnershipState::Unreachable));
        manifest.add(item("orphaned.env", OwnershipState::Orphaned));

        let existing = BTreeSet::from(["detached-missing.env".to_string()]);
        assert_eq!(
            repo_repair_exclude_paths(&manifest, &existing, |repo_id| repo_id == "repo-1"),
            vec![
                "attached.env".to_string(),
                "detached-known.env".to_string(),
                "detached-missing.env".to_string(),
            ]
        );

        assert_eq!(
            repo_repair_exclude_paths(&manifest, &BTreeSet::new(), |repo_id| repo_id == "repo-1"),
            vec!["attached.env".to_string(), "detached-known.env".to_string()]
        );
    }
}
