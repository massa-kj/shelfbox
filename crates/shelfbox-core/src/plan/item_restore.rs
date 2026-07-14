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
    link::LinkStrategy,
    ops::path::repo_relative_string,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemRestoreAction {
    RestoreFile,
    DetachKeepStore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRestorePlan {
    pub path: String,
    pub abs_path: PathBuf,
    pub store_path: PathBuf,
    pub keep_ignore: bool,
    pub action: ItemRestoreAction,
}

impl ItemRestorePlan {
    pub(crate) fn build(
        ctx: &RepoContext,
        abs_path: &Path,
        keep_ignore: bool,
        keep_store: bool,
        _link: &dyn LinkStrategy,
    ) -> Result<Self> {
        let path = repo_relative_string(&ctx.repo_root, abs_path)?;

        if keep_store {
            let item = ctx
                .manifest
                .get(&path)
                .ok_or_else(|| AppError::NotManagedLink {
                    path: abs_path.to_path_buf(),
                })?;

            return Ok(Self {
                path,
                abs_path: abs_path.to_path_buf(),
                store_path: ctx.repo_store.join(&item.store_path),
                keep_ignore,
                action: ItemRestoreAction::DetachKeepStore,
            });
        }

        let Some(item) = ctx.manifest.get(&path) else {
            return match abs_path.symlink_metadata() {
                Ok(meta) if !meta.file_type().is_symlink() => {
                    Err(AppError::RestoreDestinationExists {
                        path: abs_path.to_path_buf(),
                    })
                }
                Ok(_) | Err(_) => Err(AppError::NotManagedLink {
                    path: abs_path.to_path_buf(),
                }),
            };
        };
        let store_path = ctx.repo_store.join(&item.store_path);

        if !store_path.exists() {
            return Err(AppError::StoreMissing {
                path: abs_path.to_path_buf(),
                store_path: store_path.clone(),
            });
        }

        let repo_path =
            RepoRelativePath::new(path.clone()).ok_or_else(|| AppError::UnsafeFilesystemEntry {
                path: abs_path.to_path_buf(),
                reason: "restore repository path is not normalized",
            })?;
        let store_relative = store_path.strip_prefix(&ctx.config.store).map_err(|_| {
            AppError::UnsafeFilesystemEntry {
                path: store_path.clone(),
                reason: "restore store path escapes the configured store root",
            }
        })?;
        let store_relative =
            StoreRelativePath::new(store_relative.to_string_lossy().replace('\\', "/"))
                .ok_or_else(|| AppError::UnsafeFilesystemEntry {
                    path: store_path.clone(),
                    reason: "restore store path is not normalized",
                })?;
        let materializer =
            DefaultMaterializer::new(ctx.repo_root.clone(), ctx.config.store.clone());
        let facts = materializer.inspect(MaterializationInspectionRequest {
            location: MaterializationLocation::new(repo_path, store_relative),
            purpose: InspectionPurpose::Planning,
        })?;
        if !facts.store_regular {
            return Err(AppError::StoreMissing {
                path: abs_path.to_path_buf(),
                store_path: store_path.clone(),
            });
        }
        if !facts.store_hardlink_free || !facts.hardlink_free {
            return Err(AppError::HardlinkedFile {
                path: abs_path.to_path_buf(),
            });
        }
        match facts.repo_entry_kind {
            RepoEntryKind::ManagedSymlink => {}
            RepoEntryKind::RegularFile if facts.copy_content == CopyContentState::Equal => {}
            RepoEntryKind::RegularFile if facts.copy_content == CopyContentState::Diverged => {
                return Err(AppError::ContentDivergedRequiresSync {
                    path: abs_path.to_path_buf(),
                });
            }
            RepoEntryKind::RegularFile => {
                return Err(AppError::UnsafeFilesystemEntry {
                    path: abs_path.to_path_buf(),
                    reason: "restore regular copy could not be compared to canonical content",
                });
            }
            RepoEntryKind::Missing | RepoEntryKind::UnmanagedSymlinkOrReparsePoint => {
                return Err(AppError::NotManagedLink {
                    path: abs_path.to_path_buf(),
                });
            }
            RepoEntryKind::Directory | RepoEntryKind::Other => {
                return Err(AppError::RestoreDestinationExists {
                    path: abs_path.to_path_buf(),
                });
            }
        }

        Ok(Self {
            path,
            abs_path: abs_path.to_path_buf(),
            store_path,
            keep_ignore,
            action: ItemRestoreAction::RestoreFile,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRestoreReport {
    pub plan: ItemRestorePlan,
    pub dry_run: bool,
}
