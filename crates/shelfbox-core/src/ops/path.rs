use std::path::{Path, PathBuf};

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

    let canon_repo = std::fs::canonicalize(repo_root).ok();
    let canon_path = std::fs::canonicalize(abs_path).ok();

    if let (Some(canon_repo), Some(canon_path)) = (canon_repo, canon_path) {
        if let Ok(rel) = canon_path.strip_prefix(canon_repo) {
            return Ok(rel.to_path_buf());
        }
    }

    Err(AppError::PathOutsideRepo {
        path: abs_path.to_path_buf(),
    })
}
