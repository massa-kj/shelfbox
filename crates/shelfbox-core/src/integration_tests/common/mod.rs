/// Shared test helpers used by all integration test binaries.
///
/// Rust integration test files are separate crates; placing helpers here
/// avoids duplicating git setup code across `ops_integration.rs`,
/// `chaos_integration.rs`, and `scenario_integration.rs`.
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
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

#[allow(dead_code)]
pub fn assert_same_path(left: &Path, right: &Path) {
    assert_eq!(canonical_path(left), canonical_path(right));
}

fn canonical_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeSnapshot {
    entries: BTreeMap<String, TreeEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TreeEntry {
    Dir,
    File(String),
    Symlink(String),
}

#[allow(dead_code)]
pub fn snapshot_tree(root: &Path) -> TreeSnapshot {
    let mut snapshot = TreeSnapshot {
        entries: BTreeMap::new(),
    };
    if root.exists() {
        snapshot_tree_inner(root, root, &mut snapshot);
    }
    snapshot
}

fn snapshot_tree_inner(root: &Path, path: &Path, snapshot: &mut TreeSnapshot) {
    let mut entries: Vec<_> = std::fs::read_dir(path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();

    for entry_path in entries {
        let metadata = std::fs::symlink_metadata(&entry_path)
            .unwrap_or_else(|err| panic!("failed to stat {}: {err}", entry_path.display()));
        let rel = normalize_relative_path(root, &entry_path);

        if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(&entry_path).unwrap_or_else(|err| {
                panic!("failed to read link {}: {err}", entry_path.display())
            });
            snapshot
                .entries
                .insert(rel, TreeEntry::Symlink(normalize_path_for_display(&target)));
        } else if metadata.is_dir() {
            snapshot.entries.insert(rel, TreeEntry::Dir);
            snapshot_tree_inner(root, &entry_path, snapshot);
        } else {
            let contents = std::fs::read_to_string(&entry_path).unwrap_or_else(|_| {
                String::from_utf8_lossy(&std::fs::read(&entry_path).unwrap()).into_owned()
            });
            snapshot.entries.insert(rel, TreeEntry::File(contents));
        }
    }
}

fn normalize_relative_path(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    normalize_path_for_display(rel)
}

fn normalize_path_for_display(path: &Path) -> String {
    path.components()
        .map(|component| match component {
            Component::Normal(part) => part.to_string_lossy().into_owned(),
            Component::RootDir => String::new(),
            Component::Prefix(prefix) => prefix.as_os_str().to_string_lossy().into_owned(),
            Component::CurDir | Component::ParentDir => {
                component.as_os_str().to_string_lossy().into_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}
