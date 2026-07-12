use std::path::Path;

use crate::error::{AppError, Result};

// ── Trait ─────────────────────────────────────────────────────────────────────

/// Abstraction over different link mechanisms.
///
/// Concrete implementations: `UnixSymlinkStrategy` (Linux / macOS) and
/// `WindowsSymlinkStrategy` (Windows). Call-sites should use
/// [`DefaultLinkStrategy`] to stay platform-agnostic.
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

    /// Returns `true` if `link_path` is a link managed by shelfbox.
    ///
    /// "Managed" means: it is a link of the expected kind whose target
    /// falls inside `store_root`.
    fn is_managed_link(&self, link_path: &Path, store_root: &Path) -> bool;

    /// Returns `true` if `path` is a link of the kind this strategy manages
    /// (i.e. a symlink on both Unix and Windows).
    ///
    /// Unlike [`Self::is_managed_link`], this does **not** verify that the target
    /// falls inside the shelfbox store.
    fn is_link(&self, path: &Path) -> bool;

    /// Returns the immediate target of the link at `path`.
    ///
    /// Analogous to [`std::fs::read_link`] but routed through the strategy
    /// so that platform-specific quirks are handled in one place.
    fn read_target(&self, path: &Path) -> Result<std::path::PathBuf>;
}

impl<T: LinkStrategy + ?Sized> LinkStrategy for &T {
    fn create(&self, target: &Path, link_path: &Path) -> Result<()> {
        (*self).create(target, link_path)
    }

    fn remove(&self, link_path: &Path) -> Result<()> {
        (*self).remove(link_path)
    }

    fn is_managed_link(&self, link_path: &Path, store_root: &Path) -> bool {
        (*self).is_managed_link(link_path, store_root)
    }

    fn is_link(&self, path: &Path) -> bool {
        (*self).is_link(path)
    }

    fn read_target(&self, path: &Path) -> Result<std::path::PathBuf> {
        (*self).read_target(path)
    }
}

// ── UnixSymlinkStrategy ───────────────────────────────────────────────────────

/// [`LinkStrategy`] that uses Unix symbolic links.
///
/// Supported on Linux and macOS. Prefer [`DefaultLinkStrategy`] at call-sites
/// to remain platform-agnostic.
#[cfg(unix)]
pub struct UnixSymlinkStrategy;

#[cfg(unix)]
impl LinkStrategy for UnixSymlinkStrategy {
    fn create(&self, target: &Path, link_path: &Path) -> Result<()> {
        if let Some(parent) = link_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }
        std::os::unix::fs::symlink(target, link_path).map_err(|e| AppError::io(link_path, e))
    }

    fn remove(&self, link_path: &Path) -> Result<()> {
        std::fs::remove_file(link_path).map_err(|e| AppError::io(link_path, e))
    }

    fn is_managed_link(&self, link_path: &Path, store_root: &Path) -> bool {
        let meta = match std::fs::symlink_metadata(link_path) {
            Ok(m) => m,
            Err(_) => return false,
        };
        if !meta.file_type().is_symlink() {
            return false;
        }

        let target = match std::fs::read_link(link_path) {
            Ok(t) => t,
            Err(_) => return false,
        };

        let abs_target = if target.is_absolute() {
            target
        } else {
            match link_path.parent() {
                Some(parent) => parent.join(&target),
                None => target,
            }
        };

        let abs_target = abs_target.canonicalize().unwrap_or(abs_target);
        let store_root = store_root
            .canonicalize()
            .unwrap_or_else(|_| store_root.to_path_buf());

        abs_target.starts_with(&store_root)
    }

    fn is_link(&self, path: &Path) -> bool {
        path.symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    }

    fn read_target(&self, path: &Path) -> Result<std::path::PathBuf> {
        std::fs::read_link(path).map_err(|e| AppError::io(path, e))
    }
}

// ── WindowsSymlinkStrategy ────────────────────────────────────────────────────

/// [`LinkStrategy`] that uses Windows symbolic links.
///
/// Requires Developer Mode or an elevated shell. Prefer [`DefaultLinkStrategy`]
/// at call-sites to remain platform-agnostic. Full implementation is provided
/// in T3.
#[cfg(windows)]
pub struct WindowsSymlinkStrategy;

#[cfg(windows)]
impl LinkStrategy for WindowsSymlinkStrategy {
    fn create(&self, target: &Path, link_path: &Path) -> Result<()> {
        use std::os::windows::fs;

        if let Some(parent) = link_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }

        // Windows symlink API requires knowing whether the target is a
        // directory at creation time. shelfbox supports directory shelving,
        // so this branch must be retained for future compatibility even
        // if the current MVP primarily handles files.
        let result = if target.is_dir() {
            fs::symlink_dir(target, link_path)
        } else {
            fs::symlink_file(target, link_path)
        };

        result.map_err(|e| {
            // Windows error 1314 (ERROR_PRIVILEGE_NOT_HELD): symlink creation
            // requires Developer Mode or an elevated shell.
            if e.raw_os_error() == Some(1314) {
                AppError::Internal(
                    "Windows symlink creation is unavailable.\n\
                     Enable Windows Developer Mode or run from an elevated shell."
                        .into(),
                )
            } else {
                AppError::io(link_path, e)
            }
        })
    }

    fn remove(&self, link_path: &Path) -> Result<()> {
        std::fs::remove_file(link_path).map_err(|e| AppError::io(link_path, e))
    }

    fn is_managed_link(&self, link_path: &Path, store_root: &Path) -> bool {
        let meta = match std::fs::symlink_metadata(link_path) {
            Ok(m) => m,
            Err(_) => return false,
        };
        if !meta.file_type().is_symlink() {
            return false;
        }

        let target = match std::fs::read_link(link_path) {
            Ok(t) => t,
            Err(_) => return false,
        };

        let abs_target = if target.is_absolute() {
            target
        } else {
            match link_path.parent() {
                Some(parent) => parent.join(&target),
                None => target,
            }
        };

        let abs_target = abs_target.canonicalize().unwrap_or(abs_target);
        let store_root = store_root
            .canonicalize()
            .unwrap_or_else(|_| store_root.to_path_buf());

        abs_target.starts_with(&store_root)
    }

    fn is_link(&self, path: &Path) -> bool {
        path.symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    }

    fn read_target(&self, path: &Path) -> Result<std::path::PathBuf> {
        std::fs::read_link(path).map_err(|e| AppError::io(path, e))
    }
}

// ── DefaultLinkStrategy ───────────────────────────────────────────────────────

/// Platform-appropriate link strategy selected at compile time.
///
/// Use this type at all call-sites. All `#[cfg]` dispatch is contained
/// inside this implementation; binary call-sites stay `#[cfg]`-free.
pub struct DefaultLinkStrategy;

impl LinkStrategy for DefaultLinkStrategy {
    fn create(&self, target: &Path, link_path: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            UnixSymlinkStrategy.create(target, link_path)
        }
        #[cfg(windows)]
        {
            WindowsSymlinkStrategy.create(target, link_path)
        }
    }

    fn remove(&self, link_path: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            UnixSymlinkStrategy.remove(link_path)
        }
        #[cfg(windows)]
        {
            WindowsSymlinkStrategy.remove(link_path)
        }
    }

    fn is_managed_link(&self, link_path: &Path, store_root: &Path) -> bool {
        #[cfg(unix)]
        {
            UnixSymlinkStrategy.is_managed_link(link_path, store_root)
        }
        #[cfg(windows)]
        {
            WindowsSymlinkStrategy.is_managed_link(link_path, store_root)
        }
    }

    fn is_link(&self, path: &Path) -> bool {
        #[cfg(unix)]
        {
            UnixSymlinkStrategy.is_link(path)
        }
        #[cfg(windows)]
        {
            WindowsSymlinkStrategy.is_link(path)
        }
    }

    fn read_target(&self, path: &Path) -> Result<std::path::PathBuf> {
        #[cfg(unix)]
        {
            UnixSymlinkStrategy.read_target(path)
        }
        #[cfg(windows)]
        {
            WindowsSymlinkStrategy.read_target(path)
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn strategy() -> DefaultLinkStrategy {
        DefaultLinkStrategy
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
