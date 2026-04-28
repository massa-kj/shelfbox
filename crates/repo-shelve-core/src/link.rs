use std::path::Path;

use crate::error::{AppError, Result};

// ── Trait ─────────────────────────────────────────────────────────────────────

/// Abstraction over different link mechanisms.
///
/// The MVP ships one implementation: [`SymlinkStrategy`] (Linux / macOS).
/// Future implementations could add Windows junction or hardlink support
/// without changing any call-site in `ops/`.
pub trait LinkStrategy {
    /// Creates a link at `link_path` that points to `target`.
    ///
    /// `link_path` must not already exist; the caller is responsible for
    /// checking beforehand.
    fn create(&self, target: &Path, link_path: &Path) -> Result<()>;

    /// Removes the link at `link_path`.
    ///
    /// Only the link itself is removed; the target is not touched.
    fn remove(&self, link_path: &Path) -> Result<()>;

    /// Returns `true` if `link_path` is a link managed by repo-shelve.
    ///
    /// "Managed" means: it is a link of the expected kind whose target
    /// falls inside `store_root`.
    fn is_managed_link(&self, link_path: &Path, store_root: &Path) -> bool;
}

// ── SymlinkStrategy ───────────────────────────────────────────────────────────

/// [`LinkStrategy`] that uses Unix symbolic links.
///
/// Supported on Linux and macOS.  Not available on Windows without Developer
/// Mode, which is why Windows support is deferred to a later milestone.
pub struct SymlinkStrategy;

impl LinkStrategy for SymlinkStrategy {
    fn create(&self, target: &Path, link_path: &Path) -> Result<()> {
        // Ensure the parent directory of the link exists.
        if let Some(parent) = link_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link_path).map_err(|e| AppError::io(link_path, e))
        }

        #[cfg(not(unix))]
        {
            let _ = (target, link_path);
            Err(AppError::Internal(
                "SymlinkStrategy is only supported on Unix platforms".into(),
            ))
        }
    }

    fn remove(&self, link_path: &Path) -> Result<()> {
        // `remove_file` works on both symlinks-to-files and
        // symlinks-to-directories on Unix (unlike `remove_dir`).
        std::fs::remove_file(link_path).map_err(|e| AppError::io(link_path, e))
    }

    fn is_managed_link(&self, link_path: &Path, store_root: &Path) -> bool {
        // 1. Must be a symlink (not a regular file).
        let meta = match std::fs::symlink_metadata(link_path) {
            Ok(m) => m,
            Err(_) => return false,
        };
        if !meta.file_type().is_symlink() {
            return false;
        }

        // 2. The resolved target must live inside the repo-shelve store.
        let target = match std::fs::read_link(link_path) {
            Ok(t) => t,
            Err(_) => return false,
        };

        // Resolve relative symlinks against the directory containing the link.
        let abs_target = if target.is_absolute() {
            target
        } else {
            match link_path.parent() {
                Some(parent) => parent.join(&target),
                None => target,
            }
        };

        // Canonicalise both paths to collapse `..` before comparing, but fall
        // back to the raw absolute path if canonicalization fails (e.g. the
        // target is dangling — still owned by us if prefix matches).
        let abs_target = abs_target.canonicalize().unwrap_or(abs_target);
        let store_root = store_root
            .canonicalize()
            .unwrap_or_else(|_| store_root.to_path_buf());

        abs_target.starts_with(&store_root)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn strategy() -> SymlinkStrategy {
        SymlinkStrategy
    }

    // ── create ────────────────────────────────────────────────────────────────

    #[test]
    #[cfg(unix)]
    fn create_makes_symlink() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("target.md");
        std::fs::write(&target, "hello").unwrap();
        let link = dir.path().join("link.md");

        strategy().create(&target, &link).unwrap();

        assert!(link.is_symlink());
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "hello");
    }

    #[test]
    #[cfg(unix)]
    fn create_makes_parent_directories() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("real.md");
        std::fs::write(&target, "data").unwrap();
        let link = dir.path().join("deep").join("nested").join("link.md");

        strategy().create(&target, &link).unwrap();

        assert!(link.is_symlink());
    }

    // ── remove ────────────────────────────────────────────────────────────────

    #[test]
    #[cfg(unix)]
    fn remove_deletes_symlink_only() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("target.md");
        std::fs::write(&target, "content").unwrap();
        let link = dir.path().join("link.md");
        strategy().create(&target, &link).unwrap();

        strategy().remove(&link).unwrap();

        assert!(!link.exists());
        assert!(target.exists(), "target must not be removed");
    }

    #[test]
    #[cfg(unix)]
    fn remove_dangling_symlink_succeeds() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("gone.md");
        let link = dir.path().join("link.md");
        strategy().create(&target, &link).unwrap();
        // Target was never created, so the symlink is dangling — removal should
        // still succeed.
        strategy().remove(&link).unwrap();
        assert!(!link.exists());
    }

    // ── is_managed_link ───────────────────────────────────────────────────────

    #[test]
    #[cfg(unix)]
    fn is_managed_link_true_for_link_inside_store() {
        let store = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();

        let store_item = store.path().join("items").join("notes.md");
        std::fs::create_dir_all(store_item.parent().unwrap()).unwrap();
        std::fs::write(&store_item, "data").unwrap();

        let link = repo.path().join("notes.md");
        strategy().create(&store_item, &link).unwrap();

        assert!(strategy().is_managed_link(&link, store.path()));
    }

    #[test]
    #[cfg(unix)]
    fn is_managed_link_false_for_link_outside_store() {
        let store = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();

        let other_file = other.path().join("other.md");
        std::fs::write(&other_file, "data").unwrap();

        let link = repo.path().join("notes.md");
        strategy().create(&other_file, &link).unwrap();

        assert!(!strategy().is_managed_link(&link, store.path()));
    }

    #[test]
    fn is_managed_link_false_for_regular_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("plain.md");
        std::fs::write(&file, "hello").unwrap();

        assert!(!strategy().is_managed_link(&file, dir.path()));
    }

    #[test]
    fn is_managed_link_false_for_nonexistent_path() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("missing.md");

        assert!(!strategy().is_managed_link(&missing, dir.path()));
    }
}
