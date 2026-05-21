use std::path::{Component, Path, PathBuf};

/// Resolves `path` to an absolute path without following symlinks.
///
/// - Absolute paths are returned as-is (after normalisation).
/// - Relative paths are resolved against `cwd`.
///
/// `.` and `..` components are collapsed lexically so the result matches
/// what other parts of the code expect when comparing against `repo_root`.
pub fn resolve_path(cwd: &Path, path: &Path) -> PathBuf {
    let base = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    normalize_path(&base)
}

/// Collapses `.` and `..` components without touching the filesystem.
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}
