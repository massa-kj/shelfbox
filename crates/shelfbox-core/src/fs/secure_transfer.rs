//! Secure, bounded-memory regular-file transfer primitives.
//!
//! Operations do not call this module directly; the future materializer and
//! canonical-transfer adapters use it after their policy and journal checks.

use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
};

use ulid::Ulid;

use crate::{
    error::{AppError, Result},
    failpoint::{self, Failpoint},
    fs::{file_identity, platform},
};

const BUFFER_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PermissionMode {
    FromSource,
    PreserveDestination,
}

/// Reject an intermediate symlink, junction, or non-directory component.
/// The final component is intentionally not inspected here so callers can
/// safely use it for a create-new destination.
pub(crate) fn validate_parent_path(root: &Path, path: &Path) -> Result<()> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| AppError::UnsafeFilesystemEntry {
            path: path.to_path_buf(),
            reason: "path escapes its trusted root",
        })?;
    let mut current = root.to_path_buf();
    let parents = relative.parent().unwrap_or_else(|| Path::new(""));
    for component in parents.components() {
        let Component::Normal(name) = component else {
            return Err(AppError::UnsafeFilesystemEntry {
                path: path.to_path_buf(),
                reason: "path contains a non-normal component",
            });
        };
        current.push(name);
        let entry = platform::inspect_no_follow(&current)?;
        if entry.kind != platform::EntryKind::Directory {
            return Err(AppError::UnsafeFilesystemEntry {
                path: current,
                reason: "intermediate component is not a directory",
            });
        }
    }
    Ok(())
}

pub(crate) fn compare_regular_files(left: &Path, right: &Path) -> Result<bool> {
    let (mut left_file, left_entry) = platform::open_regular_no_follow(left)?;
    let (mut right_file, right_entry) = platform::open_regular_no_follow(right)?;
    if left_entry.link_count > 1 {
        return Err(AppError::HardlinkedFile {
            path: left.to_path_buf(),
        });
    }
    if right_entry.link_count > 1 {
        return Err(AppError::HardlinkedFile {
            path: right.to_path_buf(),
        });
    }
    let mut left_buffer = [0_u8; BUFFER_SIZE];
    let mut right_buffer = [0_u8; BUFFER_SIZE];
    loop {
        let left_read = left_file
            .read(&mut left_buffer)
            .map_err(|e| AppError::io(left, e))?;
        let right_read = right_file
            .read(&mut right_buffer)
            .map_err(|e| AppError::io(right, e))?;
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
        if left_read == 0 {
            break;
        }
    }
    file_identity::revalidate(
        left,
        file_identity::RegularFile {
            identity: left_entry.identity,
            link_count: left_entry.link_count,
        },
    )?;
    file_identity::revalidate(
        right,
        file_identity::RegularFile {
            identity: right_entry.identity,
            link_count: right_entry.link_count,
        },
    )?;
    Ok(true)
}

/// Copy an isolated source through a private create-new temp and atomically
/// replace the destination. Existing destinations are never deleted first.
pub(crate) fn copy_replace(
    source_root: &Path,
    source: &Path,
    destination_root: &Path,
    destination: &Path,
    permissions: PermissionMode,
) -> Result<()> {
    validate_parent_path(source_root, source)?;
    validate_parent_path(destination_root, destination)?;
    let source_state = file_identity::require_isolated(source)?;
    if let Ok(destination_state) = file_identity::inspect_regular(destination) {
        if destination_state.link_count > 1 {
            return Err(AppError::HardlinkedFile {
                path: destination.to_path_buf(),
            });
        }
        if destination_state.identity == source_state.identity {
            return Err(AppError::UnsafeFilesystemEntry {
                path: destination.to_path_buf(),
                reason: "source and destination are the same file",
            });
        }
    }
    let parent = destination
        .parent()
        .ok_or_else(|| AppError::UnsafeFilesystemEntry {
            path: destination.to_path_buf(),
            reason: "destination has no parent",
        })?;
    let mut temp = create_empty_private_temp(parent, destination.file_name().unwrap_or_default())?;
    let (mut input, opened) = platform::open_regular_no_follow(source)?;
    if opened.identity != source_state.identity || opened.link_count != 1 {
        return Err(AppError::FilesystemEntryChanged {
            path: source.to_path_buf(),
        });
    }
    stream_copy(&mut input, temp.file_mut(), source)?;
    temp.file_mut()
        .sync_all()
        .map_err(|e| AppError::io(temp.path(), e))?;
    match permissions {
        PermissionMode::FromSource => temp
            .file_mut()
            .set_permissions(
                fs::metadata(source)
                    .map_err(|e| AppError::io(source, e))?
                    .permissions(),
            )
            .map_err(|e| AppError::io(temp.path(), e))?,
        PermissionMode::PreserveDestination => {
            if let Ok(metadata) = fs::metadata(destination) {
                temp.file_mut()
                    .set_permissions(metadata.permissions())
                    .map_err(|e| AppError::io(temp.path(), e))?;
            }
        }
    }
    file_identity::revalidate(source, source_state)?;
    temp.commit(destination)
}

pub(crate) fn copy_replace_then_remove_source(
    source_root: &Path,
    source: &Path,
    destination_root: &Path,
    destination: &Path,
    permissions: PermissionMode,
) -> Result<()> {
    let source_state = file_identity::require_isolated(source)?;
    copy_replace(
        source_root,
        source,
        destination_root,
        destination,
        permissions,
    )?;
    file_identity::revalidate(source, source_state)?;
    fs::remove_file(source).map_err(|e| AppError::io(source, e))
}

/// Copies a regular source into a private temp whose path and identity have
/// already been durably authorized by an artifact lease.  The final component
/// is opened without following it and checked again before plaintext is
/// written, preventing a replacement of the reserved temp from receiving
/// source content.
pub(crate) fn populate_authorized_temp(
    source_root: &Path,
    source: &Path,
    temp: &Path,
    expected_temp_identity: platform::FileIdentity,
    permissions: PermissionMode,
) -> Result<()> {
    validate_parent_path(source_root, source)?;
    let source_state = file_identity::require_isolated(source)?;
    let (mut input, opened_source) = platform::open_regular_no_follow(source)?;
    if opened_source.identity != source_state.identity || opened_source.link_count != 1 {
        return Err(AppError::FilesystemEntryChanged {
            path: source.to_path_buf(),
        });
    }
    let (mut output, opened_temp) = platform::open_regular_for_write_no_follow(temp)?;
    if opened_temp.identity != expected_temp_identity || opened_temp.link_count != 1 {
        return Err(AppError::FilesystemEntryChanged {
            path: temp.to_path_buf(),
        });
    }
    output.set_len(0).map_err(|e| AppError::io(temp, e))?;
    output
        .seek(SeekFrom::Start(0))
        .map_err(|e| AppError::io(temp, e))?;
    stream_copy(&mut input, &mut output, source)?;
    output.sync_all().map_err(|e| AppError::io(temp, e))?;
    match permissions {
        PermissionMode::FromSource => output
            .set_permissions(
                fs::metadata(source)
                    .map_err(|e| AppError::io(source, e))?
                    .permissions(),
            )
            .map_err(|e| AppError::io(temp, e))?,
        PermissionMode::PreserveDestination => {}
    }
    output.sync_all().map_err(|e| AppError::io(temp, e))?;
    file_identity::revalidate(source, source_state)?;
    let current = platform::inspect_no_follow(temp)?;
    if current.identity != expected_temp_identity || current.link_count != 1 {
        return Err(AppError::FilesystemEntryChanged {
            path: temp.to_path_buf(),
        });
    }
    failpoint::after(Failpoint::PersistentMutation(
        crate::domain::copy_safety::PersistentMutation::PlaintextWrite,
    ))
}

/// Atomically installs a previously authorized temp without a delete-first
/// fallback. The caller retains the artifact record until post-commit checks
/// have succeeded.
pub(crate) fn commit_authorized_temp(
    destination_root: &Path,
    temp: &Path,
    expected_temp_identity: platform::FileIdentity,
    destination: &Path,
) -> Result<()> {
    validate_parent_path(destination_root, destination)?;
    let current = platform::inspect_no_follow(temp)?;
    if current.kind != platform::EntryKind::RegularFile
        || current.identity != expected_temp_identity
        || current.link_count != 1
    {
        return Err(AppError::FilesystemEntryChanged {
            path: temp.to_path_buf(),
        });
    }
    platform::atomic_replace(temp, destination)?;
    Ok(())
}

/// Reserves a same-directory temporary name without creating a file. The
/// journal persists this path before calling [`create_empty_private_temp_at`].
pub(crate) fn allocate_private_temp_path(
    parent: &Path,
    destination_name: &std::ffi::OsStr,
) -> Result<PathBuf> {
    for _ in 0..64 {
        let mut name = OsString::from(".");
        name.push(destination_name);
        name.push(".");
        name.push(Ulid::new().to_string());
        name.push(".tmp");
        let path = parent.join(name);
        if !path.exists() {
            return Ok(path);
        }
    }
    Err(AppError::Internal(
        "could not reserve a unique private temporary file name".into(),
    ))
}

/// Creates an empty private file at a journal-reserved path and returns its
/// no-follow identity. No plaintext may be written until the caller durably
/// records that identity.
pub(crate) fn create_empty_private_temp_at(path: &Path) -> Result<platform::FileIdentity> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .map_err(|error| AppError::io(path, error))?;
    file.sync_all().map_err(|error| AppError::io(path, error))?;
    let identity = platform::inspect_no_follow(path)?.identity;
    let parent = path
        .parent()
        .ok_or_else(|| AppError::UnsafeFilesystemEntry {
            path: path.to_path_buf(),
            reason: "temporary path has no parent",
        })?;
    platform::sync_directory(parent)?;
    drop(file);
    Ok(identity)
}

fn stream_copy(source: &mut File, destination: &mut File, source_path: &Path) -> Result<()> {
    let mut buffer = [0_u8; BUFFER_SIZE];
    loop {
        let read = source
            .read(&mut buffer)
            .map_err(|e| AppError::io(source_path, e))?;
        if read == 0 {
            return Ok(());
        }
        destination
            .write_all(&buffer[..read])
            .map_err(|e| AppError::io(source_path, e))?;
    }
}

/// A private, empty, create-new temp file. Phase 5 records its identity before
/// granting the caller permission to place plaintext in it.
pub(crate) struct PrivateTemp {
    path: PathBuf,
    file: Option<File>,
    identity: platform::FileIdentity,
    committed: bool,
}
impl PrivateTemp {
    fn create(parent: &Path, destination_name: &std::ffi::OsStr) -> Result<Self> {
        for _ in 0..64 {
            let mut name = OsString::from(".");
            name.push(destination_name);
            name.push(".");
            name.push(Ulid::new().to_string());
            name.push(".tmp");
            let path = parent.join(name);
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => {
                    let identity = platform::inspect_no_follow(&path)?.identity;
                    return Ok(Self {
                        path,
                        file: Some(file),
                        identity,
                        committed: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(AppError::io(path, error)),
            }
        }
        Err(AppError::Internal(
            "could not allocate a unique private temporary file".into(),
        ))
    }
    fn path(&self) -> &Path {
        &self.path
    }
    fn file_mut(&mut self) -> &mut File {
        self.file
            .as_mut()
            .expect("private temp file is open before commit")
    }
    fn commit(mut self, destination: &Path) -> Result<()> {
        self.file_mut()
            .sync_all()
            .map_err(|e| AppError::io(&self.path, e))?;
        drop(self.file.take());
        platform::atomic_replace(&self.path, destination)?;
        self.committed = true;
        Ok(())
    }
}

/// Allocates an empty private temp without writing plaintext. Keeping this
/// separate from transfer execution is required for the durable artifact-lease
/// protocol introduced in Phase 5.
pub(crate) fn create_empty_private_temp(
    parent: &Path,
    destination_name: &std::ffi::OsStr,
) -> Result<PrivateTemp> {
    PrivateTemp::create(parent, destination_name)
}
impl Drop for PrivateTemp {
    fn drop(&mut self) {
        if !self.committed
            && platform::inspect_no_follow(&self.path)
                .map(|e| e.identity == self.identity)
                .unwrap_or(false)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_is_bounded_and_detects_equal_and_diverged_content() {
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join("left");
        let right = dir.path().join("right");
        std::fs::write(&left, vec![b'x'; BUFFER_SIZE + 1]).unwrap();
        std::fs::write(&right, vec![b'x'; BUFFER_SIZE + 1]).unwrap();
        assert!(compare_regular_files(&left, &right).unwrap());
        std::fs::write(&right, "different").unwrap();
        assert!(!compare_regular_files(&left, &right).unwrap());
    }

    #[test]
    fn rejects_hardlinked_source_and_destination() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let alias = dir.path().join("alias");
        let destination = dir.path().join("destination");
        std::fs::write(&source, "secret").unwrap();
        std::fs::hard_link(&source, &alias).unwrap();
        assert!(matches!(
            copy_replace(
                dir.path(),
                &source,
                dir.path(),
                &destination,
                PermissionMode::FromSource
            ),
            Err(AppError::HardlinkedFile { .. })
        ));

        let isolated = dir.path().join("isolated");
        let destination_alias = dir.path().join("destination-alias");
        std::fs::write(&isolated, "isolated").unwrap();
        std::fs::write(&destination, "old").unwrap();
        std::fs::hard_link(&destination, &destination_alias).unwrap();
        assert!(matches!(
            copy_replace(
                dir.path(),
                &isolated,
                dir.path(),
                &destination,
                PermissionMode::FromSource
            ),
            Err(AppError::HardlinkedFile { .. })
        ));
    }

    #[test]
    fn rejects_intermediate_symlink_escape() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let link = root.path().join("escaped");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(outside.path(), &link).is_err() {
            return;
        }
        let path = link.join("secret");
        assert!(matches!(
            validate_parent_path(root.path(), &path),
            Err(AppError::UnsafeFilesystemEntry { .. })
        ));
    }

    #[test]
    #[cfg(unix)]
    fn private_temp_is_private_from_creation() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let temp = create_empty_private_temp(dir.path(), "destination".as_ref()).unwrap();
        assert_eq!(
            std::fs::metadata(temp.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn failed_replace_preserves_destination_and_cleans_temp() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let destination = dir.path().join("destination");
        std::fs::write(&source, "new").unwrap();
        std::fs::create_dir(&destination).unwrap();
        assert!(copy_replace(
            dir.path(),
            &source,
            dir.path(),
            &destination,
            PermissionMode::FromSource
        )
        .is_err());
        assert!(destination.is_dir());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    #[cfg(unix)]
    fn preserve_destination_permissions_never_widens_them() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let destination = dir.path().join("destination");
        std::fs::write(&source, "new").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::write(&destination, "old").unwrap();
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o600)).unwrap();

        copy_replace(
            dir.path(),
            &source,
            dir.path(),
            &destination,
            PermissionMode::PreserveDestination,
        )
        .unwrap();

        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "new");
        assert_eq!(
            std::fs::metadata(&destination)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn revalidation_detects_source_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let replacement = dir.path().join("replacement");
        std::fs::write(&source, "old").unwrap();
        let expected = file_identity::require_isolated(&source).unwrap();
        std::fs::write(&replacement, "new").unwrap();
        platform::atomic_replace(&replacement, &source).unwrap();

        assert!(matches!(
            file_identity::revalidate(&source, expected),
            Err(AppError::FilesystemEntryChanged { .. })
        ));
    }

    #[test]
    #[cfg(unix)]
    fn cross_device_transfer_removes_source_only_after_destination_is_written() {
        let shm = Path::new("/dev/shm");
        if !shm.is_dir() {
            return;
        }
        let source_dir = tempfile::tempdir().unwrap();
        let destination_dir = tempfile::Builder::new()
            .prefix("shelfbox-transfer-")
            .tempdir_in(shm)
            .unwrap();
        let source = source_dir.path().join("source");
        let destination = destination_dir.path().join("destination");
        std::fs::write(&source, "secret").unwrap();

        copy_replace_then_remove_source(
            source_dir.path(),
            &source,
            destination_dir.path(),
            &destination,
            PermissionMode::FromSource,
        )
        .unwrap();

        assert!(!source.exists());
        assert_eq!(std::fs::read_to_string(destination).unwrap(), "secret");
    }

    #[test]
    #[cfg(unix)]
    fn final_component_symlink_is_rejected_as_a_source() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("link");
        std::fs::write(&target, "secret").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(matches!(
            file_identity::require_isolated(&link),
            Err(AppError::UnsafeFilesystemEntry { .. })
        ));
    }
}
