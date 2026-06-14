use std::path::{Component, Path, PathBuf};

use shelfbox_core::{config::Config, context, ops, store::index};

/// Resolves `path` to an absolute path without following symlinks.
///
/// - Absolute paths are returned as-is (after normalisation).
/// - Relative paths are resolved against `cwd`.
///
/// `.` and `..` components are collapsed lexically so the result matches
/// what other parts of the code expect when comparing against `repo_root`.
pub fn resolve_path(cwd: &Path, path: &Path) -> PathBuf {
    let base = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    normalize_path(&base)
}

/// Collapses `.` and `..` components without touching the filesystem.
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Prints a best-effort reclaim hint when this clone has no local index match
/// but existing manifests contain positive-scoring candidates.
pub fn warn_reclaim_candidates_if_unassociated(cwd: &Path, store_override: Option<&Path>) {
    let Ok(config) = Config::load(store_override) else {
        return;
    };
    let Ok(current) = context::current_git_context(cwd) else {
        return;
    };
    let Ok(idx) = index::load(&config.store) else {
        return;
    };

    if context::resolve_existing_repo(&current, &idx).is_some() {
        return;
    }

    let Ok(candidates) = ops::reclaim::build_candidates(
        &config.store,
        &current.repo_root,
        current.remote_hint.as_deref(),
        &idx,
    ) else {
        return;
    };

    let mut matches = candidates.iter().filter(|candidate| candidate.score > 0);
    let Some(top) = matches.next() else {
        return;
    };
    let matched_count = 1 + matches.count();
    let reason = if top.reasons.is_empty() {
        "hints matched".to_string()
    } else {
        top.reasons.join(", ")
    };

    eprintln!(
        "hint: possible existing shelfbox RepoId found: {} (repos/{}, score {}, {}).",
        top.repo_id, top.repo_store_dir, top.score, reason
    );
    if matched_count == 1 {
        eprintln!(
            "hint: run `shelfbox repo reclaim --repo-id {}` to attach it explicitly, or continue to keep this clone separate.",
            top.repo_id
        );
    } else {
        eprintln!(
            "hint: {matched_count} candidates matched; run `shelfbox repo reclaim` to review and choose explicitly."
        );
    }
}
