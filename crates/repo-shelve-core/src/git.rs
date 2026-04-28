use std::path::{Path, PathBuf};

use crate::error::{AppError, Result};

/// Returns the absolute path to the root of the Git repository that contains
/// `cwd`.
///
/// Implemented by running `git rev-parse --show-toplevel`.
/// Full implementation is in Phase 3; this stub is provided so that
/// `context.rs` (Phase 2) compiles.
pub fn find_repo_root(cwd: &Path) -> Result<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        .map_err(|e| AppError::git_command(format!("failed to spawn git: {e}")))?;

    if !output.status.success() {
        return Err(AppError::NotAGitRepo);
    }

    let raw = std::str::from_utf8(&output.stdout)
        .map_err(|_| AppError::GitRootDetection("non-UTF-8 path".into()))?
        .trim();

    Ok(PathBuf::from(raw))
}

// Phase 3 will add: is_tracked, get_remote_url
