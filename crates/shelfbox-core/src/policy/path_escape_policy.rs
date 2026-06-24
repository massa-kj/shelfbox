use std::path::{Component, Path};

use crate::{
    domain::manifest::Manifest,
    error::{AppError, Result},
};

pub(crate) fn is_normalized_relative_path(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
}

pub(crate) fn is_normalized_relative_str(value: &str) -> bool {
    if value.is_empty() || value.contains('\\') || value.split('/').any(str::is_empty) {
        return false;
    }

    is_normalized_relative_path(Path::new(value))
}

pub(crate) fn is_git_internal_path(path: &Path) -> bool {
    path.components()
        .next()
        .is_some_and(|component| component.as_os_str() == ".git")
}

pub(crate) fn ensure_not_git_internal(rel_path: &Path, abs_path: &Path) -> Result<()> {
    if is_git_internal_path(rel_path) {
        return Err(AppError::PathInsideGitDir {
            path: abs_path.to_path_buf(),
        });
    }

    Ok(())
}

pub(crate) fn is_store_item_path(value: &str) -> bool {
    value
        .strip_prefix("items/")
        .is_some_and(is_normalized_relative_str)
}

pub(crate) fn validate_manifest_paths(manifest: &Manifest) -> Result<()> {
    for item in &manifest.items {
        if !is_normalized_relative_str(&item.path) {
            return Err(AppError::Internal(format!(
                "manifest item '{}' has unsafe repo-relative path '{}'",
                item.item_id, item.path
            )));
        }
        if !is_store_item_path(&item.store_path) {
            return Err(AppError::Internal(format!(
                "manifest item '{}' has unsafe store-relative path '{}'",
                item.item_id, item.store_path
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{manifest::Item, ownership::OwnershipState};

    fn item(path: &str, store_path: &str) -> Item {
        Item {
            item_id: "item-1".into(),
            origin_repo_id: "repo-1".into(),
            path: path.into(),
            store_path: store_path.into(),
            ownership_state: OwnershipState::Attached,
            created_at: "2026-04-29T00:00:00Z".into(),
            updated_at: "2026-04-29T00:00:00Z".into(),
        }
    }

    #[test]
    fn relative_strings_reject_absolute_parent_empty_and_backslash_paths() {
        assert!(is_normalized_relative_str("notes/design.md"));
        assert!(!is_normalized_relative_str(""));
        assert!(!is_normalized_relative_str("/absolute"));
        assert!(!is_normalized_relative_str("../outside"));
        assert!(!is_normalized_relative_str("notes//design.md"));
        assert!(!is_normalized_relative_str("notes\\design.md"));
    }

    #[test]
    fn git_internal_paths_are_component_based() {
        assert!(is_git_internal_path(Path::new(".git/config")));
        assert!(is_git_internal_path(Path::new(".git")));
        assert!(!is_git_internal_path(Path::new(".gitignore")));
        assert!(!is_git_internal_path(Path::new("src/.gitignore")));
    }

    #[test]
    fn store_item_paths_must_stay_beneath_items() {
        assert!(is_store_item_path("items/secrets.env"));
        assert!(is_store_item_path("items/nested/secrets.env"));
        assert!(!is_store_item_path("items"));
        assert!(!is_store_item_path("items/../secrets.env"));
        assert!(!is_store_item_path("../items/secrets.env"));
        assert!(!is_store_item_path("/tmp/secrets.env"));
    }

    #[test]
    fn manifest_paths_are_validated_together() {
        let mut manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        manifest.add(item("secrets.env", "items/secrets.env"));
        validate_manifest_paths(&manifest).unwrap();

        let mut bad_repo_path = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        bad_repo_path.add(item("../secrets.env", "items/secrets.env"));
        assert!(validate_manifest_paths(&bad_repo_path).is_err());

        let mut bad_store_path = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        bad_store_path.add(item("secrets.env", "../secrets.env"));
        assert!(validate_manifest_paths(&bad_store_path).is_err());
    }
}
