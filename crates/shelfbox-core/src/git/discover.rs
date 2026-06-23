use std::{
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    error::{AppError, Result},
    git::command::run_git,
};

pub fn find_repo_root(cwd: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
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

pub fn git_common_dir(repo_root: &Path) -> Result<PathBuf> {
    absolute_git_path(
        repo_root,
        &run_git(&["rev-parse", "--git-common-dir"], repo_root)?,
    )
}

pub fn git_dir(repo_root: &Path) -> Result<PathBuf> {
    absolute_git_path(repo_root, &run_git(&["rev-parse", "--git-dir"], repo_root)?)
}

fn absolute_git_path(repo_root: &Path, raw: &str) -> Result<PathBuf> {
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(repo_root.join(path))
    }
}
