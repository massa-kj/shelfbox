//! No-follow identity and hardlink checks shared by copy-mode primitives.

use std::path::Path;

use crate::{
    error::{AppError, Result},
    fs::platform::{self, EntryKind, FileIdentity},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RegularFile {
    pub identity: FileIdentity,
    pub link_count: u64,
}

pub(crate) fn inspect_regular(path: &Path) -> Result<RegularFile> {
    let entry = platform::inspect_no_follow(path)?;
    if entry.kind != EntryKind::RegularFile {
        return Err(AppError::UnsafeFilesystemEntry {
            path: path.to_path_buf(),
            reason: "expected a regular file without following links",
        });
    }
    Ok(RegularFile {
        identity: entry.identity,
        link_count: entry.link_count,
    })
}

pub(crate) fn require_isolated(path: &Path) -> Result<RegularFile> {
    let entry = inspect_regular(path)?;
    if entry.link_count > 1 {
        return Err(AppError::HardlinkedFile {
            path: path.to_path_buf(),
        });
    }
    Ok(entry)
}

pub(crate) fn revalidate(path: &Path, expected: RegularFile) -> Result<()> {
    if require_isolated(path)? != expected {
        return Err(AppError::FilesystemEntryChanged {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}
