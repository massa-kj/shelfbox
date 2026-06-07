/// Shared test helpers used by all integration test binaries.
///
/// Rust integration test files are separate crates; placing helpers here
/// avoids duplicating git setup code across `ops_integration.rs`,
/// `chaos_integration.rs`, and `scenario_integration.rs`.
use std::path::Path;
use std::process::Command as StdCommand;
use std::sync::OnceLock;

use tempfile::TempDir;

/// Creates a minimal Git repository (no commits) and returns the temp dir.
///
/// Suitable for most ops tests where worktrees are not needed.
#[allow(dead_code)]
pub fn init_git_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    init_git_repo_at(dir.path());
    dir
}

/// Creates a minimal Git repository with one empty commit and returns the temp
/// dir.
///
/// The empty commit is required by `git worktree add`.
#[allow(dead_code)]
pub fn init_git_repo_with_commit() -> TempDir {
    let dir = TempDir::new().unwrap();
    init_git_repo_at(dir.path());
    run_git(dir.path(), &["commit", "--allow-empty", "-m", "initial"]);
    dir
}

/// Initialises a Git repository at an arbitrary existing directory.
///
/// Used when the caller controls the directory lifecycle (e.g. for rename
/// scenarios where `TempDir` must not manage the path directly).
#[allow(dead_code)]
pub fn init_git_repo_at(path: &Path) {
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Test User"],
    ] {
        run_git(path, &args);
    }
}

/// Runs a git command and panics on failure.
#[allow(dead_code)]
pub fn run_git(cwd: &Path, args: &[&str]) {
    let out = StdCommand::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn git {}: {e}", args[0]));
    assert!(
        out.status.success(),
        "git {} failed: {}",
        args[0],
        String::from_utf8_lossy(&out.stderr)
    );
}

fn try_create_file_symlink(target: &Path, link_path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link_path)
    }

    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link_path)
    }
}

/// Returns `true` when the current environment can create file symlinks.
#[allow(dead_code)]
pub fn require_symlink_support() -> bool {
    static SYMLINK_SUPPORT_ERROR: OnceLock<Option<String>> = OnceLock::new();

    let unsupported_reason = SYMLINK_SUPPORT_ERROR.get_or_init(|| {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("symlink-target.txt");
        let link_path = dir.path().join("symlink-link.txt");
        std::fs::write(&target, "probe").unwrap();

        match try_create_file_symlink(&target, &link_path) {
            Ok(()) => {
                let _ = std::fs::remove_file(&link_path);
                None
            }
            Err(err) => Some(format!(
                "skipping symlink-dependent integration test because symlink creation is unavailable: {err}"
            )),
        }
    });

    if let Some(reason) = unsupported_reason {
        if std::env::var_os("SHELFBOX_REQUIRE_SYMLINKS").is_some() {
            panic!("{reason}");
        }
        eprintln!("{reason}");
        return false;
    }

    true
}

/// Creates a file symlink in a platform-aware way for integration tests.
#[allow(dead_code)]
pub fn create_file_symlink(target: &Path, link_path: &Path) {
    try_create_file_symlink(target, link_path).unwrap_or_else(|e| {
        panic!(
            "failed to create symlink {} -> {}: {e}",
            link_path.display(),
            target.display()
        )
    });
}
