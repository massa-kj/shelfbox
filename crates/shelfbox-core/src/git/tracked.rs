use std::{collections::HashSet, path::Path, process::Command};

use crate::error::{AppError, Result};

pub fn is_tracked(repo_root: &Path, path: &Path) -> Result<bool> {
    let output = Command::new("git")
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(path)
        .current_dir(repo_root)
        .output()
        .map_err(|e| AppError::git_command(format!("failed to spawn git: {e}")))?;

    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => {
            let stderr = std::str::from_utf8(&output.stderr)
                .unwrap_or("")
                .trim()
                .to_string();
            Err(AppError::git_command(format!(
                "`git ls-files --error-unmatch` failed unexpectedly: {stderr}"
            )))
        }
    }
}

pub fn tracked_files_in_dir(repo_root: &Path, abs_dir: &Path) -> Result<HashSet<String>> {
    let output = Command::new("git")
        .args(["ls-files", "--"])
        .arg(abs_dir)
        .current_dir(repo_root)
        .output()
        .map_err(|e| AppError::git_command(format!("failed to spawn git: {e}")))?;

    if !output.status.success() {
        let stderr = std::str::from_utf8(&output.stderr)
            .unwrap_or("")
            .trim()
            .to_string();
        return Err(AppError::git_command(format!(
            "`git ls-files` failed: {stderr}"
        )));
    }

    let stdout = std::str::from_utf8(&output.stdout)
        .map_err(|_| AppError::git_command("git output is not valid UTF-8"))?;

    Ok(stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}
