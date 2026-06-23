use std::{
    io::Write,
    path::{Path, PathBuf},
};

use crate::{
    error::{AppError, Result},
    fs::permissions,
};

pub(crate) use crate::fs::permissions::ParentDirMode;

pub(crate) fn write(path: &Path, contents: impl AsRef<[u8]>, mode: ParentDirMode) -> Result<()> {
    let tmp_path = temp_path_for(path);
    write_with_temp_path(path, &tmp_path, contents, mode)
}

pub(crate) fn write_with_temp_path(
    path: &Path,
    tmp_path: &Path,
    contents: impl AsRef<[u8]>,
    mode: ParentDirMode,
) -> Result<()> {
    permissions::ensure_parent_dir(path, mode)?;

    {
        let mut file = std::fs::File::create(tmp_path).map_err(|e| AppError::io(tmp_path, e))?;
        file.write_all(contents.as_ref())
            .map_err(|e| AppError::io(tmp_path, e))?;
    }

    std::fs::rename(tmp_path, path).map_err(|e| AppError::io(path, e))?;
    Ok(())
}

fn temp_path_for(path: &Path) -> PathBuf {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => path.with_extension(format!("{ext}.tmp")),
        None => path.with_extension("tmp"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_replaces_file_and_removes_default_temp_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.json");

        write(&path, b"first", ParentDirMode::Private).unwrap();
        write(&path, b"second", ParentDirMode::Private).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        assert!(!dir.path().join("index.json.tmp").exists());
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
}
