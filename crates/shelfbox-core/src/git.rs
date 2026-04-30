use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{AppError, Result};

// ── Internal helper ──────────────────────────────────────────────────────────

/// Runs a `git` subprocess rooted at `repo_root` and returns its trimmed
/// stdout on success.  Returns an `AppError::GitCommand` with the combined
/// stderr output when the process exits non-zero.
fn run_git(args: &[&str], repo_root: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_root)
        // Prevent git from reading the user's global config in tests.
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

// ── Public API ────────────────────────────────────────────────────────────────

/// Returns the absolute path to the root of the Git repository that contains
/// `cwd`.
///
/// Runs `git rev-parse --show-toplevel`.
pub fn find_repo_root(cwd: &Path) -> Result<PathBuf> {
    // Use run_git through a manual Command here because we don't yet have a
    // repo_root to pass; cwd serves that role.
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

/// Returns `true` if `path` is currently tracked by Git.
///
/// Runs `git ls-files --error-unmatch -- <path>`.
/// A non-zero exit code means the file is not tracked (which is the normal
/// case for files that shelfbox manages).
///
/// `path` must be absolute or relative to `repo_root`.
pub fn is_tracked(repo_root: &Path, path: &Path) -> Result<bool> {
    let output = Command::new("git")
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(path)
        .current_dir(repo_root)
        .output()
        .map_err(|e| AppError::git_command(format!("failed to spawn git: {e}")))?;

    // Exit 0  → file is tracked.
    // Exit 1  → file is not tracked (not an error we should propagate).
    // Other   → unexpected git error.
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

/// Returns the `origin` remote URL for the repository, or `None` if no
/// remote named `origin` is configured.
///
/// Used only for informational purposes in the manifest; a missing remote
/// must never block any operation.
pub fn get_remote_url(repo_root: &Path) -> Result<Option<String>> {
    match run_git(&["remote", "get-url", "origin"], repo_root) {
        Ok(url) if !url.is_empty() => Ok(Some(url)),
        Ok(_) => Ok(None),
        // "origin" doesn't exist → treat as no remote, not an error.
        Err(AppError::GitCommand { .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;
    use tempfile::TempDir;

    /// Creates a minimal Git repository in a temp directory and returns its path.
    fn init_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        StdCommand::new("git")
            .args(["init", "-b", "main"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        // Configure identity so `git commit` works in CI environments.
        StdCommand::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    /// Commits an empty file so the repo has a HEAD and the file is tracked.
    fn commit_file(repo: &Path, name: &str) {
        let file_path = repo.join(name);
        std::fs::write(&file_path, "hello").unwrap();
        StdCommand::new("git")
            .args(["add", name])
            .current_dir(repo)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-m", "test commit"])
            .current_dir(repo)
            .output()
            .unwrap();
    }

    #[test]
    fn find_repo_root_inside_repo() {
        let dir = init_repo();
        // Create a subdirectory and run from there to verify traversal.
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();

        let root = find_repo_root(&sub).unwrap();
        assert_eq!(
            root.canonicalize().unwrap(),
            dir.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn find_repo_root_outside_repo() {
        // Use a directory that is guaranteed not to be a git repo.
        let dir = TempDir::new().unwrap();
        let err = find_repo_root(dir.path()).unwrap_err();
        assert!(matches!(err, AppError::NotAGitRepo));
    }

    #[test]
    fn is_tracked_returns_true_for_committed_file() {
        let dir = init_repo();
        commit_file(dir.path(), "tracked.md");
        assert!(is_tracked(dir.path(), Path::new("tracked.md")).unwrap());
    }

    #[test]
    fn is_tracked_returns_false_for_untracked_file() {
        let dir = init_repo();
        std::fs::write(dir.path().join("untracked.md"), "hi").unwrap();
        assert!(!is_tracked(dir.path(), Path::new("untracked.md")).unwrap());
    }

    #[test]
    fn is_tracked_returns_false_for_staged_only_file() {
        let dir = init_repo();
        let file = dir.path().join("staged.md");
        std::fs::write(&file, "staged").unwrap();
        StdCommand::new("git")
            .args(["add", "staged.md"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        // `git ls-files --error-unmatch` checks the index; staged files ARE
        // in the index, so they count as tracked.
        assert!(is_tracked(dir.path(), Path::new("staged.md")).unwrap());
    }

    #[test]
    fn get_remote_url_returns_none_for_repo_without_remote() {
        let dir = init_repo();
        let result = get_remote_url(dir.path()).unwrap();
        assert!(result.is_none());
    }
}
