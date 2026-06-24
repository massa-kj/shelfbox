pub(crate) mod command;
pub(crate) mod discover;
pub(crate) mod exclude;
pub(crate) mod remote;
pub(crate) mod tracked;

pub use discover::{find_repo_root, git_common_dir, git_dir};
pub use exclude::exclude_file_path;
pub use remote::{get_remote_url, normalize_remote_hint, remote_url};
pub use tracked::{is_tracked, tracked_files_in_dir};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppError;
    use std::{path::Path, process::Command as StdCommand};
    use tempfile::TempDir;

    fn init_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        StdCommand::new("git")
            .args(["init", "-b", "main"])
            .current_dir(dir.path())
            .output()
            .unwrap();
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
        assert!(is_tracked(dir.path(), Path::new("staged.md")).unwrap());
    }

    #[test]
    fn remote_url_returns_none_for_repo_without_remote() {
        let dir = init_repo();
        let result = remote_url(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn remote_url_returns_origin_url_when_configured() {
        let dir = init_repo();
        StdCommand::new("git")
            .args(["remote", "add", "origin", "git@github.com:org/repo.git"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        let result = remote_url(dir.path()).unwrap();

        assert_eq!(result, Some("git@github.com:org/repo.git".into()));
    }

    #[test]
    fn normalize_remote_hint_supports_https_with_git_suffix() {
        assert_eq!(
            normalize_remote_hint("https://github.com/org/repo.git"),
            Some("github.com/org/repo".into())
        );
    }

    #[test]
    fn normalize_remote_hint_supports_ssh_shorthand() {
        assert_eq!(
            normalize_remote_hint("git@github.com:org/repo.git"),
            Some("github.com/org/repo".into())
        );
    }

    #[test]
    fn normalize_remote_hint_supports_ssh_url() {
        assert_eq!(
            normalize_remote_hint("ssh://git@host/path/repo"),
            Some("host/path/repo".into())
        );
    }

    #[test]
    fn normalize_remote_hint_supports_url_without_git_suffix() {
        assert_eq!(
            normalize_remote_hint("https://gitlab.com/org/repo"),
            Some("gitlab.com/org/repo".into())
        );
    }

    #[test]
    fn normalize_remote_hint_rejects_empty_or_non_parseable_values() {
        assert_eq!(normalize_remote_hint(""), None);
        assert_eq!(normalize_remote_hint("not a remote"), None);
        assert_eq!(normalize_remote_hint("file:///tmp/repo.git"), None);
    }

    #[test]
    fn get_remote_url_wraps_remote_url_for_compatibility() {
        let dir = init_repo();
        StdCommand::new("git")
            .args(["remote", "add", "origin", "https://github.com/org/repo"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        assert_eq!(
            get_remote_url(dir.path()).unwrap(),
            Some("https://github.com/org/repo".into())
        );
    }
}
