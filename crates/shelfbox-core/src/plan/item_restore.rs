use std::path::{Path, PathBuf};

use crate::{
    context::RepoContext,
    error::{AppError, Result},
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
        link: &dyn LinkStrategy,
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

        match std::fs::symlink_metadata(abs_path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                if !link.is_managed_link(abs_path, &ctx.config.store) {
                    return Err(AppError::NotManagedLink {
                        path: abs_path.to_path_buf(),
                    });
                }
            }
            Ok(_) => {
                return Err(AppError::RestoreDestinationExists {
                    path: abs_path.to_path_buf(),
                });
            }
            Err(_) => {
                return Err(AppError::NotManagedLink {
                    path: abs_path.to_path_buf(),
                });
            }
        }

        let store_path = ctx
            .manifest
            .get(&path)
            .map(|item| ctx.repo_store.join(&item.store_path))
            .ok_or_else(|| {
                AppError::Internal(format!(
                    "symlink at '{}' points into store but is not recorded in the manifest",
                    abs_path.display()
                ))
            })?;

        if !store_path.exists() {
            return Err(AppError::StoreMissing {
                path: abs_path.to_path_buf(),
                store_path: store_path.clone(),
            });
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
