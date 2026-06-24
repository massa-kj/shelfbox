use std::path::{Path, PathBuf};

use crate::{
    error::{AppError, Result},
    policy::path_escape_policy,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AddEntryKind {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectoryCandidateDecision {
    Add,
    SkipAlreadyManaged,
    SkipGitTracked,
    SkipSymlink,
    StoreConflict,
}

pub(crate) fn validate_add_location(rel_path: &Path, abs_path: &Path) -> Result<()> {
    path_escape_policy::ensure_not_git_internal(rel_path, abs_path)
}

pub(crate) fn validate_add_entry_kind(abs_path: &Path, kind: AddEntryKind) -> Result<()> {
    match kind {
        AddEntryKind::File => Ok(()),
        AddEntryKind::Directory => Err(AppError::PathIsDirectory {
            path: abs_path.to_path_buf(),
        }),
        AddEntryKind::Symlink => Err(AppError::PathIsSymlink {
            path: abs_path.to_path_buf(),
        }),
    }
}

pub(crate) fn validate_add_git_state(abs_path: &Path, is_tracked: bool) -> Result<()> {
    if is_tracked {
        return Err(AppError::PathIsTracked {
            path: abs_path.to_path_buf(),
        });
    }

    Ok(())
}

pub(crate) fn validate_add_manifest_state(abs_path: &Path, already_managed: bool) -> Result<()> {
    if already_managed {
        return Err(AppError::AlreadyManaged {
            path: abs_path.to_path_buf(),
        });
    }

    Ok(())
}

pub(crate) fn validate_add_store_destination(
    store_path: &Path,
    store_path_exists: bool,
) -> Result<()> {
    if store_path_exists {
        return Err(AppError::StoreConflict {
            store_path: store_path.to_path_buf(),
        });
    }

    Ok(())
}

pub(crate) fn classify_directory_candidate(
    already_managed: bool,
    is_symlink: bool,
    is_tracked: bool,
    store_path_exists: bool,
) -> DirectoryCandidateDecision {
    if already_managed {
        DirectoryCandidateDecision::SkipAlreadyManaged
    } else if is_symlink {
        DirectoryCandidateDecision::SkipSymlink
    } else if is_tracked {
        DirectoryCandidateDecision::SkipGitTracked
    } else if store_path_exists {
        DirectoryCandidateDecision::StoreConflict
    } else {
        DirectoryCandidateDecision::Add
    }
}

pub(crate) fn store_item_path_for_repo_path(rel_path: &str) -> String {
    format!("items/{rel_path}")
}

pub(crate) fn add_entry_kind_from_meta(meta: &std::fs::Metadata) -> AddEntryKind {
    let file_type = meta.file_type();
    if file_type.is_symlink() {
        AddEntryKind::Symlink
    } else if meta.is_dir() {
        AddEntryKind::Directory
    } else {
        AddEntryKind::File
    }
}

pub(crate) fn conflict_message(store_path: PathBuf) -> String {
    format!("store conflict: '{}' already exists", store_path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_locations_reject_git_internal_paths_only_by_component() {
        let abs = Path::new("/repo/.git/config");
        assert!(matches!(
            validate_add_location(Path::new(".git/config"), abs),
            Err(AppError::PathInsideGitDir { .. })
        ));

        validate_add_location(Path::new(".gitignore"), Path::new("/repo/.gitignore")).unwrap();
    }

    #[test]
    fn add_entry_kind_rejects_symlinks_and_directories() {
        let path = Path::new("/repo/secrets.env");
        validate_add_entry_kind(path, AddEntryKind::File).unwrap();
        assert!(matches!(
            validate_add_entry_kind(path, AddEntryKind::Symlink),
            Err(AppError::PathIsSymlink { .. })
        ));
        assert!(matches!(
            validate_add_entry_kind(path, AddEntryKind::Directory),
            Err(AppError::PathIsDirectory { .. })
        ));
    }

    #[test]
    fn directory_candidate_classification_preserves_existing_priority() {
        assert_eq!(
            classify_directory_candidate(true, true, true, true),
            DirectoryCandidateDecision::SkipAlreadyManaged
        );
        assert_eq!(
            classify_directory_candidate(false, true, true, true),
            DirectoryCandidateDecision::SkipSymlink
        );
        assert_eq!(
            classify_directory_candidate(false, false, true, true),
            DirectoryCandidateDecision::SkipGitTracked
        );
        assert_eq!(
            classify_directory_candidate(false, false, false, true),
            DirectoryCandidateDecision::StoreConflict
        );
        assert_eq!(
            classify_directory_candidate(false, false, false, false),
            DirectoryCandidateDecision::Add
        );
    }

    #[test]
    fn store_item_paths_are_deterministic() {
        assert_eq!(
            store_item_path_for_repo_path("nested/secrets.env"),
            "items/nested/secrets.env"
        );
    }
}
