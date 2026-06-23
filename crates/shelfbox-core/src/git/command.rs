use std::{path::Path, process::Command};

use crate::error::{AppError, Result};

pub(crate) fn run_git(args: &[&str], repo_root: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()
        .map_err(|e| AppError::git_command(format!("failed to spawn git: {e}")))?;

    if output.status.success() {
        let stdout = std::str::from_utf8(&output.stdout)
            .map_err(|_| AppError::git_command("git output is not valid UTF-8"))?
            .trim()
            .to_string();
        Ok(stdout)
    } else {
        let stderr = std::str::from_utf8(&output.stderr)
            .unwrap_or("")
            .trim()
            .to_string();
        Err(AppError::git_command(format!(
            "`git {}` failed: {}",
            args.join(" "),
            stderr
        )))
    }
}
