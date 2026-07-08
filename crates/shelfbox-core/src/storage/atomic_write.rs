use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

use ulid::Ulid;

use crate::{
    error::{AppError, Result},
    fs::{permissions, platform},
};

pub(crate) use crate::fs::permissions::ParentDirMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TempFileMode {
    Default,
    Private,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // D3 option surface; operation records will request this in Phase 2.
pub(crate) enum FileSyncMode {
    Skip,
    BeforeRename,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // D3 option surface; operation records will request this in Phase 2.
pub(crate) enum ParentDirectorySyncMode {
    Skip,
    BestEffort,
    Require,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplacementMode {
    Atomic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TempPathMode {
    Generated,
    CallerProvided(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AtomicWriteOptions {
    parent_dir_mode: ParentDirMode,
    temp_file_mode: TempFileMode,
    temp_path_mode: TempPathMode,
    file_sync: FileSyncMode,
    parent_directory_sync: ParentDirectorySyncMode,
    replacement: ReplacementMode,
}

#[allow(dead_code)] // D3 option surface; operation records will request this in Phase 2.
impl AtomicWriteOptions {
    pub(crate) fn new(parent_dir_mode: ParentDirMode) -> Self {
        let temp_file_mode = match parent_dir_mode {
            ParentDirMode::Default => TempFileMode::Default,
            ParentDirMode::Private => TempFileMode::Private,
        };

        Self {
            parent_dir_mode,
            temp_file_mode,
            temp_path_mode: TempPathMode::Generated,
            file_sync: FileSyncMode::Skip,
            parent_directory_sync: ParentDirectorySyncMode::Skip,
            replacement: ReplacementMode::Atomic,
        }
    }

    pub(crate) fn with_temp_file_mode(mut self, mode: TempFileMode) -> Self {
        self.temp_file_mode = mode;
        self
    }

    pub(crate) fn with_temp_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.temp_path_mode = TempPathMode::CallerProvided(path.into());
        self
    }

    pub(crate) fn with_file_sync(mut self, mode: FileSyncMode) -> Self {
        self.file_sync = mode;
        self
    }

    pub(crate) fn with_parent_directory_sync(mut self, mode: ParentDirectorySyncMode) -> Self {
        self.parent_directory_sync = mode;
        self
    }
}

pub(crate) fn write(path: &Path, contents: impl AsRef<[u8]>, mode: ParentDirMode) -> Result<()> {
    write_with_options(path, contents, AtomicWriteOptions::new(mode))
}

#[allow(dead_code)] // D3 option surface; fixed temp paths are opt-in for explicit callers.
pub(crate) fn write_with_temp_path(
    path: &Path,
    tmp_path: &Path,
    contents: impl AsRef<[u8]>,
    mode: ParentDirMode,
) -> Result<()> {
    write_with_options(
        path,
        contents,
        AtomicWriteOptions::new(mode)
            .with_temp_file_mode(TempFileMode::Default)
            .with_temp_path(tmp_path),
    )
}

pub(crate) fn write_with_options(
    path: &Path,
    contents: impl AsRef<[u8]>,
    options: AtomicWriteOptions,
) -> Result<()> {
    permissions::ensure_parent_dir(path, options.parent_dir_mode)?;
    ensure_temp_is_in_destination_parent(path, &options.temp_path_mode)?;
    preflight_required_parent_sync(path, options.parent_directory_sync)?;

    let mut temp = create_temp_file(path, &options)?;
    temp.write_all(contents.as_ref())?;
    if options.file_sync == FileSyncMode::BeforeRename {
        temp.sync_all()?;
    }
    temp.replace_destination(path, options.replacement)?;
    sync_parent_after_rename(path, options.parent_directory_sync)
}

fn create_temp_file(path: &Path, options: &AtomicWriteOptions) -> Result<CreatedTempFile> {
    match &options.temp_path_mode {
        TempPathMode::Generated => {
            for _ in 0..64 {
                let temp_path = generated_temp_path_for(path)?;
                match CreatedTempFile::create(&temp_path, options.temp_file_mode) {
                    Ok(temp) => return Ok(temp),
                    Err(AppError::Io { source, .. })
                        if source.kind() == ErrorKind::AlreadyExists =>
                    {
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }

            Err(AppError::Internal(format!(
                "could not allocate a unique temporary path for {}",
                path.display()
            )))
        }
        TempPathMode::CallerProvided(temp_path) => {
            CreatedTempFile::create(temp_path, options.temp_file_mode)
        }
    }
}

fn ensure_temp_is_in_destination_parent(path: &Path, temp_path_mode: &TempPathMode) -> Result<()> {
    let TempPathMode::CallerProvided(temp_path) = temp_path_mode else {
        return Ok(());
    };

    if parent_dir(path) != parent_dir(temp_path) {
        return Err(AppError::Internal(format!(
            "atomic write temp path must be in the destination directory: '{}' and '{}'",
            temp_path.display(),
            path.display()
        )));
    }

    Ok(())
}

fn preflight_required_parent_sync(path: &Path, mode: ParentDirectorySyncMode) -> Result<()> {
    if mode == ParentDirectorySyncMode::Require {
        platform::sync_directory(parent_dir(path))?;
    }

    Ok(())
}

fn sync_parent_after_rename(path: &Path, mode: ParentDirectorySyncMode) -> Result<()> {
    match mode {
        ParentDirectorySyncMode::Skip => Ok(()),
        ParentDirectorySyncMode::BestEffort => {
            let _ = platform::sync_directory(parent_dir(path));
            Ok(())
        }
        ParentDirectorySyncMode::Require => platform::sync_directory(parent_dir(path)),
    }
}

fn generated_temp_path_for(path: &Path) -> Result<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        AppError::Internal(format!(
            "atomic write destination must have a file name: {}",
            path.display()
        ))
    })?;

    let mut temp_name = OsString::from(".");
    temp_name.push(file_name);
    temp_name.push(".");
    temp_name.push(Ulid::new().to_string());
    temp_name.push(".tmp");

    Ok(parent_dir(path).join(temp_name))
}

fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

struct CreatedTempFile {
    path: PathBuf,
    file: Option<File>,
    identity: platform::FileIdentity,
    cleanup_on_drop: bool,
}

impl CreatedTempFile {
    fn create(path: &Path, mode: TempFileMode) -> Result<Self> {
        let file = open_temp_file(path, mode)?;
        let identity = platform::inspect_no_follow(path)?.identity;

        Ok(Self {
            path: path.to_path_buf(),
            file: Some(file),
            identity,
            cleanup_on_drop: true,
        })
    }

    fn write_all(&mut self, contents: &[u8]) -> Result<()> {
        self.file
            .as_mut()
            .expect("temp file should be open before replacement")
            .write_all(contents)
            .map_err(|error| AppError::io(&self.path, error))
    }

    fn sync_all(&self) -> Result<()> {
        self.file
            .as_ref()
            .expect("temp file should be open before replacement")
            .sync_all()
            .map_err(|error| AppError::io(&self.path, error))
    }

    fn replace_destination(&mut self, destination: &Path, mode: ReplacementMode) -> Result<()> {
        self.close();

        match mode {
            ReplacementMode::Atomic => atomic_replace(&self.path, destination)?,
        }

        self.cleanup_on_drop = false;
        Ok(())
    }

    fn close(&mut self) {
        drop(self.file.take());
    }

    fn cleanup_if_owned(&mut self) {
        if !self.cleanup_on_drop {
            return;
        }
        self.close();

        if let Ok(entry) = platform::inspect_no_follow(&self.path) {
            if entry.identity == self.identity {
                let _ = std::fs::remove_file(&self.path);
            }
        }
        self.cleanup_on_drop = false;
    }
}

impl Drop for CreatedTempFile {
    fn drop(&mut self) {
        self.cleanup_if_owned();
    }
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> Result<()> {
    platform::atomic_replace(source, destination)
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> Result<()> {
    let source = absolute_path_with_existing_parent(source)?;
    let destination = absolute_path_with_existing_parent(destination)?;
    platform::atomic_replace(&source, &destination)
}

#[cfg(windows)]
fn absolute_path_with_existing_parent(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    let file_name = path.file_name().ok_or_else(|| {
        AppError::Internal(format!(
            "atomic write path must have a file name: {}",
            path.display()
        ))
    })?;
    let parent = parent_dir(path);
    let absolute_parent =
        std::fs::canonicalize(parent).map_err(|error| AppError::io(parent, error))?;

    Ok(absolute_parent.join(file_name))
}

fn open_temp_file(path: &Path, mode: TempFileMode) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        if mode == TempFileMode::Private {
            options.mode(0o600);
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }

    let _ = mode;
    options
        .open(path)
        .map_err(|error| AppError::io(path, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_replaces_file_and_removes_generated_temp_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.json");

        write(&path, b"first", ParentDirMode::Private).unwrap();
        write(&path, b"second", ParentDirMode::Private).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn write_with_temp_path_preserves_caller_temp_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let tmp_path = dir.path().join(".shelfbox-config-write.tmp");

        write_with_temp_path(
            &path,
            &tmp_path,
            "store = \"/tmp/shelfbox\"\n",
            ParentDirMode::Default,
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "store = \"/tmp/shelfbox\"\n"
        );
        assert!(!tmp_path.exists());
    }

    #[test]
    fn caller_temp_path_must_not_already_exist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let tmp_path = dir.path().join(".shelfbox-config-write.tmp");
        std::fs::write(&tmp_path, "do not touch").unwrap();

        let error = write_with_temp_path(
            &path,
            &tmp_path,
            "store = \"/tmp/shelfbox\"\n",
            ParentDirMode::Default,
        )
        .unwrap_err();

        assert!(matches!(error, AppError::Io { .. }));
        assert!(!path.exists());
        assert_eq!(std::fs::read_to_string(&tmp_path).unwrap(), "do not touch");
    }

    #[test]
    fn failed_replacement_cleans_generated_temp_without_touching_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("destination");
        std::fs::create_dir(&path).unwrap();

        assert!(write(&path, b"contents", ParentDirMode::Private).is_err());

        assert!(path.is_dir());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    #[cfg(unix)]
    fn private_temp_creation_produces_private_destination_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");

        write(&path, b"{}", ParentDirMode::Private).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    #[cfg(unix)]
    fn durable_options_sync_file_and_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("operation-record.json");

        write_with_options(
            &path,
            b"{\"phase\":\"prepared\"}",
            AtomicWriteOptions::new(ParentDirMode::Private)
                .with_file_sync(FileSyncMode::BeforeRename)
                .with_parent_directory_sync(ParentDirectorySyncMode::Require),
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"phase\":\"prepared\"}"
        );
    }

    #[test]
    fn best_effort_parent_sync_does_not_block_basic_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.json");

        write_with_options(
            &path,
            b"{}",
            AtomicWriteOptions::new(ParentDirMode::Private)
                .with_parent_directory_sync(ParentDirectorySyncMode::BestEffort),
        )
        .unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
    }

    #[test]
    #[cfg(windows)]
    fn required_parent_sync_fails_before_creating_destination_on_windows() {
        use crate::error::FilesystemCapability;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("operation-record.json");

        let error = write_with_options(
            &path,
            b"{}",
            AtomicWriteOptions::new(ParentDirMode::Private)
                .with_parent_directory_sync(ParentDirectorySyncMode::Require),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            AppError::FilesystemCapabilityUnavailable {
                capability: FilesystemCapability::DirectoryDurability,
                ..
            }
        ));
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
