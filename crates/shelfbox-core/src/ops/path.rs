use std::path::{Path, PathBuf};
use std::{ffi::OsString, fs};

use crate::error::{AppError, Result};

/// Converts an absolute path to a repo-relative path.
///
/// Fast path uses lexical `strip_prefix`. If that fails, a canonicalized
/// fallback handles platform alias paths (e.g. `/var` vs `/private/var` on
/// macOS).
pub(crate) fn repo_relative_path(repo_root: &Path, abs_path: &Path) -> Result<PathBuf> {
    if let Ok(rel) = abs_path.strip_prefix(repo_root) {
        return Ok(rel.to_path_buf());
    }

    let canon_repo = canonicalize_with_missing_tail(repo_root);
    let canon_path = canonicalize_with_missing_tail(abs_path);

    if let (Some(canon_repo), Some(canon_path)) = (canon_repo, canon_path) {
        if let Ok(rel) = canon_path.strip_prefix(canon_repo) {
            return Ok(rel.to_path_buf());
        }
    }

    Err(AppError::PathOutsideRepo {
        path: abs_path.to_path_buf(),
    })
}

fn canonicalize_with_missing_tail(path: &Path) -> Option<PathBuf> {
    if let Ok(canon) = fs::canonicalize(path) {
        return Some(canon);
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
