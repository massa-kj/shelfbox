//! Private platform filesystem adapter.
//!
//! D1 freezes the capability boundary and validates the operating-system
//! primitives. Phase 2 composes these primitives into secure transfer and
//! materialization operations. Operation modules must never import this module.

use std::{fs::File, path::Path};

use crate::error::{AppError, FilesystemCapability, Result};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
use windows as imp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CapabilitySupport {
    /// The supported platform API provides the required semantics.
    Supported,
    /// The API exists, but the mounted filesystem can reject the operation.
    RuntimeChecked,
    /// No documented platform primitive provides the required guarantee.
    Unsupported(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlatformCapabilities {
    pub no_follow_inspection: CapabilitySupport,
    pub stable_file_identity: CapabilitySupport,
    pub link_count: CapabilitySupport,
    pub atomic_replace_regular_file: CapabilitySupport,
    pub atomic_replace_symlink_or_reparse_point: CapabilitySupport,
    pub directory_durability: CapabilitySupport,
}

impl PlatformCapabilities {
    pub(crate) fn support(self, capability: FilesystemCapability) -> CapabilitySupport {
        match capability {
            FilesystemCapability::NoFollowInspection => self.no_follow_inspection,
            FilesystemCapability::StableFileIdentity => self.stable_file_identity,
            FilesystemCapability::LinkCount => self.link_count,
            FilesystemCapability::AtomicReplaceRegularFile => self.atomic_replace_regular_file,
            FilesystemCapability::AtomicReplaceSymlinkOrReparsePoint => {
                self.atomic_replace_symlink_or_reparse_point
            }
            FilesystemCapability::DirectoryDurability => self.directory_durability,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FileIdentity {
    /// Device ID on Unix; volume serial number on Windows.
    pub volume: u64,
    /// Inode encoded into 16 bytes on Unix; 128-bit file ID on Windows.
    pub file: [u8; 16],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryKind {
    RegularFile,
    Directory,
    SymlinkOrReparsePoint,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InspectedEntry {
    pub kind: EntryKind,
    pub identity: FileIdentity,
    pub link_count: u64,
}

pub(crate) const fn capabilities() -> PlatformCapabilities {
    imp::CAPABILITIES
}

pub(crate) fn inspect_no_follow(path: &Path) -> Result<InspectedEntry> {
    imp::inspect_no_follow(path)
}

pub(crate) fn open_regular_no_follow(path: &Path) -> Result<(File, InspectedEntry)> {
    imp::open_regular_no_follow(path)
}

/// Opens an existing regular file for writing without following its final
/// component.  Callers compare the returned identity with their durable temp
/// record before writing any plaintext.
pub(crate) fn open_regular_for_write_no_follow(path: &Path) -> Result<(File, InspectedEntry)> {
    imp::open_regular_for_write_no_follow(path)
}

/// Atomically renames a prepared regular file over an existing non-directory
/// entry in the same directory.
///
/// Failure must preserve both the source and old destination. There is no
/// delete-then-create fallback.
pub(crate) fn atomic_replace(source: &Path, destination: &Path) -> Result<()> {
    let capability = match inspect_no_follow(destination) {
        Ok(entry) if entry.kind == EntryKind::SymlinkOrReparsePoint => {
            FilesystemCapability::AtomicReplaceSymlinkOrReparsePoint
        }
        Ok(_) => FilesystemCapability::AtomicReplaceRegularFile,
        Err(AppError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            FilesystemCapability::AtomicReplaceRegularFile
        }
        Err(error) => return Err(error),
    };
    require(capability)?;
    imp::atomic_replace(source, destination)
}

/// Confirms that replacement stays in one physical directory. Path spelling is
/// not sufficient on platforms that expose the same directory through aliases
/// (for example, `/var` and `/private/var` on macOS), so compare no-follow
/// directory identities instead.
pub(super) fn require_same_parent_directory(source: &Path, destination: &Path) -> Result<()> {
    let source_parent = source.parent();
    let destination_parent = destination.parent();
    let (Some(source_parent), Some(destination_parent)) = (source_parent, destination_parent)
    else {
        return Err(AppError::Internal(format!(
            "atomic replacement requires source and destination parents: '{}' and '{}'",
            source.display(),
            destination.display()
        )));
    };

    let source_entry = inspect_no_follow(source_parent)?;
    let destination_entry = inspect_no_follow(destination_parent)?;
    if source_entry.kind != EntryKind::Directory
        || destination_entry.kind != EntryKind::Directory
        || source_entry.identity != destination_entry.identity
    {
        return Err(AppError::Internal(format!(
            "atomic replacement requires source and destination in the same directory: '{}' and '{}'",
            source.display(),
            destination.display()
        )));
    }
    Ok(())
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    require(FilesystemCapability::DirectoryDurability)?;
    imp::sync_directory(path)
}

fn require(capability: FilesystemCapability) -> Result<()> {
    match capabilities().support(capability) {
        CapabilitySupport::Supported | CapabilitySupport::RuntimeChecked => Ok(()),
        CapabilitySupport::Unsupported(reason) => Err(AppError::FilesystemCapabilityUnavailable {
            capability,
            platform: std::env::consts::OS,
            reason,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn capability_matrix_is_explicit() {
        let matrix = capabilities();
        for capability in [
            FilesystemCapability::NoFollowInspection,
            FilesystemCapability::StableFileIdentity,
            FilesystemCapability::LinkCount,
            FilesystemCapability::AtomicReplaceRegularFile,
            FilesystemCapability::AtomicReplaceSymlinkOrReparsePoint,
            FilesystemCapability::DirectoryDurability,
        ] {
            let _ = matrix.support(capability);
        }
    }

    #[test]
    fn no_follow_inspection_and_link_count_use_entry_identity() {
        let dir = TempDir::new().unwrap();
        let original = dir.path().join("original");
        let hardlink = dir.path().join("hardlink");
        std::fs::write(&original, "secret").unwrap();
        std::fs::hard_link(&original, &hardlink).unwrap();

        let original_entry = inspect_no_follow(&original).unwrap();
        let hardlink_entry = inspect_no_follow(&hardlink).unwrap();

        assert_eq!(original_entry.kind, EntryKind::RegularFile);
        assert_eq!(original_entry.identity, hardlink_entry.identity);
        assert!(original_entry.link_count >= 2);
        assert!(hardlink_entry.link_count >= 2);
    }

    #[test]
    fn atomic_replace_existing_regular_file() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("prepared");
        let destination = dir.path().join("destination");
        std::fs::write(&source, "new").unwrap();
        std::fs::write(&destination, "old").unwrap();

        atomic_replace(&source, &destination).unwrap();

        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "new");
        assert!(!source.exists());
    }

    #[cfg(unix)]
    #[test]
    fn atomic_replace_accepts_alias_of_the_same_parent_directory() {
        let parent = TempDir::new().unwrap();
        let directory = parent.path().join("directory");
        let alias_parent = TempDir::new().unwrap();
        let alias = alias_parent.path().join("alias");
        std::fs::create_dir(&directory).unwrap();
        std::os::unix::fs::symlink(parent.path(), &alias).unwrap();

        let source = alias.join("directory/prepared");
        let destination = directory.join("destination");
        std::fs::write(&source, "new").unwrap();
        std::fs::write(&destination, "old").unwrap();

        atomic_replace(&source, &destination).unwrap();

        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "new");
        assert!(!source.exists());
    }

    #[cfg(unix)]
    #[test]
    fn atomic_replace_rejects_a_final_symlink_parent() {
        let parent = TempDir::new().unwrap();
        let directory = parent.path().join("directory");
        let symlink_parent = parent.path().join("symlink-parent");
        std::fs::create_dir(&directory).unwrap();
        std::os::unix::fs::symlink(&directory, &symlink_parent).unwrap();

        let source = symlink_parent.join("prepared");
        let destination = directory.join("destination");
        std::fs::write(&source, "new").unwrap();
        std::fs::write(&destination, "old").unwrap();

        assert!(atomic_replace(&source, &destination).is_err());
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "old");
    }

    #[test]
    fn atomic_replace_symlink_does_not_touch_target() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("prepared");
        let target = dir.path().join("target");
        let destination = dir.path().join("destination");
        std::fs::write(&source, "new").unwrap();
        std::fs::write(&target, "target-content").unwrap();
        if !create_file_symlink(&target, &destination) {
            return;
        }

        let before = inspect_no_follow(&destination).unwrap();
        let target_entry = inspect_no_follow(&target).unwrap();
        assert_eq!(before.kind, EntryKind::SymlinkOrReparsePoint);
        assert_ne!(
            before.identity, target_entry.identity,
            "inspection must describe the link entry, not its target"
        );

        atomic_replace(&source, &destination).unwrap();

        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "target-content");
        assert!(!destination
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn failed_replace_preserves_source_and_old_destination() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("prepared");
        let destination = dir.path().join("destination-directory");
        std::fs::write(&source, "new").unwrap();
        std::fs::create_dir(&destination).unwrap();

        assert!(atomic_replace(&source, &destination).is_err());
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "new");
        assert!(destination.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn directory_sync_primitive_succeeds() {
        let dir = TempDir::new().unwrap();
        sync_directory(dir.path()).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn directory_durability_fails_with_typed_capability_error() {
        let dir = TempDir::new().unwrap();
        assert!(matches!(
            sync_directory(dir.path()),
            Err(AppError::FilesystemCapabilityUnavailable {
                capability: FilesystemCapability::DirectoryDurability,
                ..
            })
        ));
    }

    #[cfg(unix)]
    fn create_file_symlink(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).unwrap();
        true
    }

    #[cfg(windows)]
    fn create_file_symlink(target: &Path, link: &Path) -> bool {
        match std::os::windows::fs::symlink_file(target, link) {
            Ok(()) => true,
            Err(error) if std::env::var_os("SHELFBOX_REQUIRE_SYMLINKS").is_none() => {
                eprintln!("skipping symlink test: {error}");
                false
            }
            Err(error) => panic!("symlink support is required: {error}"),
        }
    }
}
