use std::{
    ffi::CString,
    fs::File,
    os::fd::FromRawFd,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::Path,
};

use crate::{
    error::{AppError, Result},
    fs::platform::{
        CapabilitySupport, EntryKind, FileIdentity, InspectedEntry, PlatformCapabilities,
    },
};

pub(super) const CAPABILITIES: PlatformCapabilities = PlatformCapabilities {
    no_follow_inspection: CapabilitySupport::Supported,
    stable_file_identity: CapabilitySupport::Supported,
    link_count: CapabilitySupport::Supported,
    atomic_replace_regular_file: CapabilitySupport::Supported,
    atomic_replace_symlink_or_reparse_point: CapabilitySupport::Supported,
    directory_durability: CapabilitySupport::RuntimeChecked,
};

pub(super) fn inspect_no_follow(path: &Path) -> Result<InspectedEntry> {
    let file = open_entry_no_follow(path)?;
    let metadata = file.metadata().map_err(|error| AppError::io(path, error))?;
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        EntryKind::SymlinkOrReparsePoint
    } else if file_type.is_file() {
        EntryKind::RegularFile
    } else if file_type.is_dir() {
        EntryKind::Directory
    } else {
        EntryKind::Other
    };

    let mut file_id = [0_u8; 16];
    file_id[..8].copy_from_slice(&metadata.ino().to_le_bytes());

    Ok(InspectedEntry {
        kind,
        identity: FileIdentity {
            volume: metadata.dev(),
            file: file_id,
        },
        link_count: metadata.nlink(),
    })
}

pub(super) fn open_regular_no_follow(path: &Path) -> Result<(File, InspectedEntry)> {
    open_regular(path, libc::O_RDONLY)
}

pub(super) fn open_regular_for_write_no_follow(path: &Path) -> Result<(File, InspectedEntry)> {
    open_regular(path, libc::O_WRONLY)
}

fn open_regular(path: &Path, access: i32) -> Result<(File, InspectedEntry)> {
    let path_bytes = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        AppError::Internal(format!("path contains an interior NUL: {}", path.display()))
    })?;
    let flags = access | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let descriptor = unsafe { libc::open(path_bytes.as_ptr(), flags) };
    if descriptor < 0 {
        return Err(AppError::io(path, std::io::Error::last_os_error()));
    }
    let file = unsafe { File::from_raw_fd(descriptor) };
    let metadata = file.metadata().map_err(|error| AppError::io(path, error))?;
    if !metadata.file_type().is_file() {
        return Err(AppError::UnsafeFilesystemEntry {
            path: path.to_path_buf(),
            reason: "expected a regular file",
        });
    }
    let mut file_id = [0_u8; 16];
    file_id[..8].copy_from_slice(&metadata.ino().to_le_bytes());
    Ok((
        file,
        InspectedEntry {
            kind: EntryKind::RegularFile,
            identity: FileIdentity {
                volume: metadata.dev(),
                file: file_id,
            },
            link_count: metadata.nlink(),
        },
    ))
}

pub(super) fn atomic_replace(source: &Path, destination: &Path) -> Result<()> {
    atomic_replace_with(source, destination, |source, destination| {
        std::fs::rename(source, destination)
    })
}

fn atomic_replace_with(
    source: &Path,
    destination: &Path,
    rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    require_same_parent(source, destination)?;
    rename(source, destination).map_err(|error| AppError::io(destination, error))
}

pub(super) fn sync_directory(path: &Path) -> Result<()> {
    let directory = open_directory_no_follow(path)?;
    directory
        .sync_all()
        .map_err(|error| AppError::io(path, error))
}

fn open_entry_no_follow(path: &Path) -> Result<File> {
    let path_bytes = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        AppError::Internal(format!("path contains an interior NUL: {}", path.display()))
    })?;

    #[cfg(target_os = "linux")]
    let flags = libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    #[cfg(target_os = "macos")]
    let flags = libc::O_SYMLINK | libc::O_NOFOLLOW | libc::O_CLOEXEC;

    // SAFETY: `path_bytes` is NUL-terminated and remains alive for the call.
    // The returned descriptor is checked before ownership moves to `File`.
    let descriptor = unsafe { libc::open(path_bytes.as_ptr(), flags) };
    if descriptor < 0 {
        return Err(AppError::io(path, std::io::Error::last_os_error()));
    }

    // SAFETY: `descriptor` is a newly opened, owned descriptor.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

fn open_directory_no_follow(path: &Path) -> Result<File> {
    let path_bytes = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        AppError::Internal(format!("path contains an interior NUL: {}", path.display()))
    })?;
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

    // SAFETY: see `open_entry_no_follow`.
    let descriptor = unsafe { libc::open(path_bytes.as_ptr(), flags) };
    if descriptor < 0 {
        return Err(AppError::io(path, std::io::Error::last_os_error()));
    }

    // SAFETY: `descriptor` is a newly opened, owned descriptor.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

fn require_same_parent(source: &Path, destination: &Path) -> Result<()> {
    if source.parent() != destination.parent() {
        return Err(AppError::Internal(format!(
            "atomic replacement requires source and destination in the same directory: '{}' and '{}'",
            source.display(),
            destination.display()
        )));
    }
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn forced_exdev_preserves_source_and_old_destination() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("prepared");
        let destination = dir.path().join("destination");
        std::fs::write(&source, "new").unwrap();
        std::fs::write(&destination, "old").unwrap();

        let error = atomic_replace_with(&source, &destination, |_, _| {
            Err(std::io::Error::from_raw_os_error(libc::EXDEV))
        })
        .unwrap_err();

        match error {
            AppError::Io { source: error, .. } => {
                assert_eq!(error.raw_os_error(), Some(libc::EXDEV));
            }
            other => panic!("expected EXDEV I/O error, got {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "old");
    }
}
