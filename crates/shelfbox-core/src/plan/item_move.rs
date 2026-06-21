use std::path::{Path, PathBuf};

use crate::{
    context::RepoContext,
    error::{AppError, Result},
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

        if new_abs.exists() {
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

        match std::fs::read_link(old_abs) {
            Ok(target) if target == old_store_path => {}
            _ => {
                return Err(AppError::MoveSourceSymlinkMismatch {
                    path: old_abs.to_path_buf(),
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
