use std::{
    fs::{File, OpenOptions},
    mem::{size_of, zeroed},
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, RawHandle},
        prelude::OsStrExt,
    },
    path::Path,
    ptr,
};

use windows_sys::Win32::{
    Foundation::{ERROR_INVALID_FUNCTION, ERROR_NOT_SUPPORTED},
    Storage::FileSystem::{
        FileIdInfo, FileRenameInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
        SetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_ID_INFO, FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        SYNCHRONIZE,
    },
};

use crate::{
    error::{AppError, FilesystemCapability, Result},
    fs::platform::{
        CapabilitySupport, EntryKind, FileIdentity, InspectedEntry, PlatformCapabilities,
    },
};

pub(super) const CAPABILITIES: PlatformCapabilities = PlatformCapabilities {
    no_follow_inspection: CapabilitySupport::Supported,
    stable_file_identity: CapabilitySupport::RuntimeChecked,
    link_count: CapabilitySupport::Supported,
    atomic_replace_regular_file: CapabilitySupport::RuntimeChecked,
    atomic_replace_symlink_or_reparse_point: CapabilitySupport::RuntimeChecked,
    directory_durability: CapabilitySupport::Unsupported(
        "Windows exposes file-buffer flushing but no documented parent-directory fsync equivalent",
    ),
};

pub(super) fn inspect_no_follow(path: &Path) -> Result<InspectedEntry> {
    let file = open_no_follow(path, 0)?;
    inspect_handle(&file, path)
}

pub(super) fn atomic_replace(source: &Path, destination: &Path) -> Result<()> {
    require_same_parent(source, destination)?;
    if !destination.is_absolute() {
        return Err(AppError::Internal(format!(
            "Windows atomic replacement requires an absolute destination: {}",
            destination.display()
        )));
    }

    let source_file = open_no_follow(source, DELETE | SYNCHRONIZE)?;
    let source_entry = inspect_handle(&source_file, source)?;
    if source_entry.kind != EntryKind::RegularFile {
        return Err(AppError::Internal(format!(
            "atomic replacement source is not a regular file: {}",
            source.display()
        )));
    }

    let destination_kind = inspect_no_follow(destination).ok().map(|entry| entry.kind);
    rename_handle_over_path(&source_file, destination).map_err(|error| {
        if matches!(
            error.raw_os_error().map(|code| code as u32),
            Some(ERROR_INVALID_FUNCTION) | Some(ERROR_NOT_SUPPORTED)
        ) {
            let capability = if destination_kind == Some(EntryKind::SymlinkOrReparsePoint) {
                FilesystemCapability::AtomicReplaceSymlinkOrReparsePoint
            } else {
                FilesystemCapability::AtomicReplaceRegularFile
            };
            AppError::FilesystemCapabilityUnavailable {
                capability,
                platform: "windows",
                reason: "the mounted filesystem rejected handle-based replacement",
            }
        } else {
            AppError::io(destination, error)
        }
    })
}

pub(super) fn sync_directory(_path: &Path) -> Result<()> {
    Err(AppError::FilesystemCapabilityUnavailable {
        capability: FilesystemCapability::DirectoryDurability,
        platform: "windows",
        reason: "Windows exposes file-buffer flushing but no documented parent-directory fsync equivalent",
    })
}

fn open_no_follow(path: &Path, access_mode: u32) -> Result<File> {
    OpenOptions::new()
        .access_mode(access_mode)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .map_err(|error| AppError::io(path, error))
}

fn inspect_handle(file: &File, path: &Path) -> Result<InspectedEntry> {
    let handle = raw_handle(file);

    // SAFETY: both buffers are valid for the duration of their respective
    // calls and `handle` belongs to the open `File`.
    let mut basic: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    if unsafe { GetFileInformationByHandle(handle, &mut basic) } == 0 {
        return Err(AppError::io(path, std::io::Error::last_os_error()));
    }

    // FILE_ID_INFO provides a 128-bit identifier, avoiding the documented
    // uniqueness limitation of the older 64-bit file index on ReFS.
    let mut identity: FILE_ID_INFO = unsafe { zeroed() };
    if unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            ptr::addr_of_mut!(identity).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        let error = std::io::Error::last_os_error();
        if matches!(
            error.raw_os_error().map(|code| code as u32),
            Some(ERROR_INVALID_FUNCTION) | Some(ERROR_NOT_SUPPORTED)
        ) {
            return Err(AppError::FilesystemCapabilityUnavailable {
                capability: FilesystemCapability::StableFileIdentity,
                platform: "windows",
                reason: "the mounted filesystem does not expose FILE_ID_INFO",
            });
        }
        return Err(AppError::io(path, error));
    }

    let kind = if basic.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        EntryKind::SymlinkOrReparsePoint
    } else if basic.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        EntryKind::Directory
    } else {
        EntryKind::RegularFile
    };

    Ok(InspectedEntry {
        kind,
        identity: FileIdentity {
            volume: identity.VolumeSerialNumber,
            file: identity.FileId.Identifier,
        },
        link_count: u64::from(basic.nNumberOfLinks),
    })
}

fn rename_handle_over_path(source: &File, destination: &Path) -> std::io::Result<()> {
    let name: Vec<u16> = destination.as_os_str().encode_wide().collect();
    // `FILE_RENAME_INFO` already includes the first UTF-16 code unit. Add the
    // complete path length as prescribed by the API; this leaves room for the
    // trailing NUL and any C-layout tail padding.
    let byte_len = size_of::<FILE_RENAME_INFO>() + name.len() * size_of::<u16>();
    let word_count = byte_len.div_ceil(size_of::<usize>());
    let mut buffer = vec![0_usize; word_count];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();

    // SAFETY: `buffer` is pointer-aligned and large enough for the fixed
    // header, full UTF-16 path, and trailing NUL. The OS reads it only for the
    // duration of the call.
    unsafe {
        (*info).Anonymous.ReplaceIfExists = true;
        (*info).RootDirectory = ptr::null_mut();
        (*info).FileNameLength = (name.len() * size_of::<u16>()) as u32;
        ptr::copy_nonoverlapping(name.as_ptr(), (*info).FileName.as_mut_ptr(), name.len());

        if SetFileInformationByHandle(
            raw_handle(source),
            FileRenameInfo,
            info.cast(),
            byte_len as u32,
        ) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }

    Ok(())
}

fn raw_handle(file: &File) -> windows_sys::Win32::Foundation::HANDLE {
    file.as_raw_handle() as RawHandle
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

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;

    #[test]
    fn sharing_violation_preserves_source_and_old_destination() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("prepared");
        let destination = dir.path().join("destination");
        std::fs::write(&source, "new").unwrap();
        std::fs::write(&destination, "old").unwrap();

        let _held = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&destination)
            .unwrap();

        let error = atomic_replace(&source, &destination).unwrap_err();
        match error {
            AppError::Io { source: error, .. } => assert_eq!(
                error.raw_os_error().map(|code| code as u32),
                Some(ERROR_SHARING_VIOLATION)
            ),
            other => panic!("expected sharing violation, got {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "old");
    }
}
