use std::path::{Path, PathBuf};
use std::{ffi::OsString, fs};

use crate::{
    error::{AppError, Result},
    policy::path_escape_policy,
};

/// Converts an absolute path to a repo-relative path.
///
/// Fast path uses lexical `strip_prefix`. If that fails, a canonicalized
/// fallback handles platform alias paths (e.g. `/var` vs `/private/var` on
/// macOS).
pub(crate) fn repo_relative_path(repo_root: &Path, abs_path: &Path) -> Result<PathBuf> {
    if let Ok(rel) = abs_path.strip_prefix(repo_root) {
        if is_normalized_relative(rel) {
            return Ok(rel.to_path_buf());
        }
    }

    let canon_repo = canonicalize_with_missing_tail(repo_root);
    let canon_path = canonicalize_with_missing_tail(abs_path);

    if let (Some(canon_repo), Some(canon_path)) = (canon_repo, canon_path) {
        if let Ok(rel) = canon_path.strip_prefix(canon_repo) {
            if is_normalized_relative(rel) {
                return Ok(rel.to_path_buf());
            }
        }
    }

    Err(AppError::PathOutsideRepo {
        path: abs_path.to_path_buf(),
    })
}

/// Converts an absolute path to a repo-relative string using forward slashes.
pub(crate) fn repo_relative_string(repo_root: &Path, abs_path: &Path) -> Result<String> {
    repo_relative_path(repo_root, abs_path).map(|rel| normalize_repo_relative(&rel))
}

/// Normalizes a repo-relative path for manifest and ignore file usage.
pub(crate) fn normalize_repo_relative(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn is_normalized_relative(path: &Path) -> bool {
    path_escape_policy::is_normalized_relative_path(path)
}

fn canonicalize_with_missing_tail(path: &Path) -> Option<PathBuf> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.file_type().is_symlink() {
            if let Ok(canon) = fs::canonicalize(path) {
                return Some(canon);
            }
        }
    }

    let mut tail: Vec<OsString> = Vec::new();
    let mut cursor = path;

    while let Some(parent) = cursor.parent() {
        if let Some(name) = cursor.file_name() {
            tail.push(name.to_os_string());
        }

        if let Ok(mut canon_parent) = fs::canonicalize(parent) {
            for segment in tail.iter().rev() {
                canon_parent.push(segment);
            }
            return Some(canon_parent);
        }

        cursor = parent;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_relative_path_accepts_absolute_path_inside_repo() {
        let repo = tempfile::tempdir().unwrap();
        let file = repo.path().join("notes/design.md");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "draft").unwrap();

        assert_eq!(
            repo_relative_path(repo.path(), &file).unwrap(),
            PathBuf::from("notes/design.md")
        );
    }

    #[test]
    fn repo_relative_path_rejects_path_outside_repo() {
        let repo = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file = outside.path().join("secret.txt");
        std::fs::write(&file, "nope").unwrap();

        assert!(matches!(
            repo_relative_path(repo.path(), &file),
            Err(AppError::PathOutsideRepo { .. })
        ));
    }

    #[test]
    fn repo_relative_path_handles_parent_components() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let sibling = root.path().join("sibling");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();

        assert!(matches!(
            repo_relative_path(&repo, &repo.join("../sibling/file.txt")),
            Err(AppError::PathOutsideRepo { .. })
        ));
    }

    #[test]
    fn repo_relative_path_handles_missing_tail_inside_repo() {
        let repo = tempfile::tempdir().unwrap();
        let missing = repo.path().join("future/deep/file.md");

        assert_eq!(
            repo_relative_path(repo.path(), &missing).unwrap(),
            PathBuf::from("future/deep/file.md")
        );
    }

    #[test]
    fn repo_relative_string_uses_forward_slashes() {
        assert_eq!(
            normalize_repo_relative(Path::new("notes").join("design.md").as_path()),
            "notes/design.md"
        );
    }
}
