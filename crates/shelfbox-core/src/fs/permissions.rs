use std::path::Path;

use crate::error::{AppError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParentDirMode {
    Default,
    Private,
}

pub(crate) fn ensure_parent_dir(path: &Path, mode: ParentDirMode) -> Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    ensure_dir(parent, mode)
}

pub(crate) fn ensure_dir(path: &Path, mode: ParentDirMode) -> Result<()> {
    match mode {
        ParentDirMode::Default => {
            std::fs::create_dir_all(path).map_err(|e| AppError::io(path, e))?;
        }
        ParentDirMode::Private => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(path)
                    .map_err(|e| AppError::io(path, e))?;
            }
            #[cfg(not(unix))]
            {
                std::fs::create_dir_all(path).map_err(|e| AppError::io(path, e))?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_parent_dir_creation_creates_missing_parent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.toml");

        ensure_parent_dir(&path, ParentDirMode::Default).unwrap();

        assert!(dir.path().join("nested").is_dir());
    }

    #[test]
    #[cfg(unix)]
    fn private_parent_dir_creation_uses_private_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private/index.json");

        ensure_parent_dir(&path, ParentDirMode::Private).unwrap();

        let mode = std::fs::metadata(dir.path().join("private"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }
}
