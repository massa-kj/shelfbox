use std::path::{Path, PathBuf};

use crate::{
    context::RepoContext,
    domain::{
        materialization::CopyContentState,
        path::{RepoRelativePath, StoreRelativePath},
    },
    error::{AppError, Result},
    fs::materializer::{
        DefaultMaterializer, InspectionPurpose, MaterializationInspectionRequest,
        MaterializationLocation, Materializer, RepoEntryKind,
    },
    git,
    ops::path::{normalize_repo_relative, repo_relative_path},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemMovePlan {
    pub old_path: String,
    pub new_path: String,
    pub old_abs_path: PathBuf,
    pub new_abs_path: PathBuf,
    pub old_store_path: PathBuf,
    pub new_store_path: PathBuf,
    pub old_store_path_relative: String,
    pub new_store_path_relative: String,
}

impl ItemMovePlan {
    pub(crate) fn build(ctx: &RepoContext, old_abs: &Path, new_abs: &Path) -> Result<Self> {
        let old_rel = repo_relative_path(&ctx.repo_root, old_abs)?;
        let new_rel = repo_relative_path(&ctx.repo_root, new_abs)?;

        let old_path = normalize_repo_relative(&old_rel);
        let new_path = normalize_repo_relative(&new_rel);

        let item = ctx
            .manifest
            .get(&old_path)
            .ok_or_else(|| AppError::NotManagedLink {
                path: old_abs.to_path_buf(),
            })?
            .clone();

        if ctx.manifest.contains(&new_path) {
            return Err(AppError::AlreadyManaged {
                path: new_abs.to_path_buf(),
            });
        }

        // `Path::exists` follows symlinks and therefore misses a dangling
        // destination.  The materializer's no-follow facts below are the
        // authoritative destination observation.
        if new_abs.symlink_metadata().is_ok() {
            return Err(AppError::MoveDestinationExists {
                path: new_abs.to_path_buf(),
            });
        }

        if git::is_tracked(&ctx.repo_root, new_abs)? {
            return Err(AppError::PathIsTracked {
                path: new_abs.to_path_buf(),
            });
        }

        let old_store_path_relative = item.store_path;
        let old_store_path = ctx.repo_store.join(&old_store_path_relative);
        let new_store_path_relative = format!("items/{new_path}");
        let new_store_path = ctx.repo_store.join(&new_store_path_relative);

        if !old_store_path.exists() {
            return Err(AppError::StoreMissing {
                path: old_abs.to_path_buf(),
                store_path: old_store_path.clone(),
            });
        }

        let old_repo = RepoRelativePath::new(old_path.clone()).ok_or_else(|| {
            AppError::UnsafeFilesystemEntry {
                path: old_abs.to_path_buf(),
                reason: "move source path is not normalized",
            }
        })?;
        let old_store_relative = store_relative(&ctx.config.store, &old_store_path)?;
        let materializer =
            DefaultMaterializer::new(ctx.repo_root.clone(), ctx.config.store.clone());
        let facts = materializer.inspect(MaterializationInspectionRequest {
            location: MaterializationLocation::new(old_repo, old_store_relative),
            purpose: InspectionPurpose::Planning,
        })?;
        if !facts.store_regular || !facts.store_hardlink_free || !facts.hardlink_free {
            return Err(AppError::HardlinkedFile {
                path: old_abs.to_path_buf(),
            });
        }
        match facts.repo_entry_kind {
            RepoEntryKind::ManagedSymlink => {}
            RepoEntryKind::RegularFile if facts.copy_content == CopyContentState::Equal => {}
            RepoEntryKind::RegularFile if facts.copy_content == CopyContentState::Diverged => {
                return Err(AppError::ContentDivergedRequiresSync {
                    path: old_abs.to_path_buf(),
                });
            }
            RepoEntryKind::UnmanagedSymlinkOrReparsePoint => {
                return Err(AppError::MoveSourceSymlinkMismatch {
                    path: old_abs.to_path_buf(),
                });
            }
            RepoEntryKind::Missing => {
                return Err(AppError::NotManagedLink {
                    path: old_abs.to_path_buf(),
                });
            }
            _ => {
                return Err(AppError::UnsafeFilesystemEntry {
                    path: old_abs.to_path_buf(),
                    reason: "move source is not an isolated managed materialization",
                });
            }
        }

        Ok(Self {
            old_path,
            new_path,
            old_abs_path: old_abs.to_path_buf(),
            new_abs_path: new_abs.to_path_buf(),
            old_store_path,
            new_store_path,
            old_store_path_relative,
            new_store_path_relative,
        })
    }
}

fn store_relative(store_root: &Path, absolute: &Path) -> Result<StoreRelativePath> {
    let relative =
        absolute
            .strip_prefix(store_root)
            .map_err(|_| AppError::UnsafeFilesystemEntry {
                path: absolute.to_path_buf(),
                reason: "move store path escapes the configured store root",
            })?;
    StoreRelativePath::new(relative.to_string_lossy().replace('\\', "/")).ok_or_else(|| {
        AppError::UnsafeFilesystemEntry {
            path: absolute.to_path_buf(),
            reason: "move store path is not normalized",
        }
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemMoveWarning {
    ExcludeRemoveFailed { path: String, message: String },
    ExcludeAddFailed { path: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemMoveReport {
    pub plan: ItemMovePlan,
    pub dry_run: bool,
    pub warnings: Vec<ItemMoveWarning>,
}
