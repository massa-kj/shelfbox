use std::path::{Path, PathBuf};

use crate::error::{AppError, Result};

pub fn index_path(store_root: &Path) -> PathBuf {
    store_root.join("index.json")
}

pub fn meta_path(store_root: &Path) -> PathBuf {
    store_root.join("meta.json")
}

pub fn repos_dir(store_root: &Path) -> PathBuf {
    store_root.join("repos")
}

pub fn repo_store_path(store_root: &Path, repo_store_dir: &str) -> PathBuf {
    repos_dir(store_root).join(repo_store_dir)
}

pub fn manifest_path(repo_store: &Path) -> PathBuf {
    repo_store.join("manifest.json")
}

pub fn lock_path(store_root: &Path) -> PathBuf {
    store_root.join(".lock")
}

pub fn allocate_repo_store_dir(
    store_root: &Path,
    base_name: &str,
    repo_id: &str,
) -> Result<String> {
    let repos_dir = repos_dir(store_root);
    let mut n = 1;
    loop {
        let candidate = if n == 1 {
            base_name.to_string()
        } else {
            format!("{base_name}-{n}")
        };

        if repo_store_dir_available(&repos_dir, &candidate, repo_id)? {
            return Ok(candidate);
        }
        n += 1;
    }
}

pub fn sanitize_repo_store_name(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "repo".into()
    } else {
        slug
    }
}

fn repo_store_dir_available(repos_dir: &Path, candidate: &str, repo_id: &str) -> Result<bool> {
    let repo_store = repos_dir.join(candidate);
    if !repo_store.exists() {
        return Ok(true);
    }

    let manifest_path = manifest_path(&repo_store);
    let contents = match std::fs::read_to_string(&manifest_path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(AppError::io(manifest_path, e)),
    };
    let raw: serde_json::Value =
        serde_json::from_str(&contents).map_err(|e| AppError::json(manifest_path, e))?;

    Ok(raw.get("repo_id").and_then(|v| v.as_str()) == Some(repo_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::manifest_store::{self, Manifest};

    #[test]
    fn repo_store_name_is_sanitized_without_ulid() {
        assert_eq!(sanitize_repo_store_name("my project"), "my-project");
        assert_eq!(sanitize_repo_store_name("!!!"), "repo");
    }

    #[test]
    fn new_repo_store_dir_uses_sanitized_name_without_ulid() {
        let store = tempfile::TempDir::new().unwrap();

        let dir = allocate_repo_store_dir(store.path(), "my-project", "repo-1").unwrap();

        assert_eq!(dir, "my-project");
    }

    #[test]
    fn repo_store_dir_conflict_uses_numeric_suffix() {
        let store = tempfile::TempDir::new().unwrap();
        let existing = store.path().join("repos/my-project");
        let manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        manifest_store::save(&existing, &manifest).unwrap();

        let dir = allocate_repo_store_dir(store.path(), "my-project", "repo-2").unwrap();

        assert_eq!(dir, "my-project-2");
    }

    #[test]
    fn repo_store_dir_three_way_conflict_uses_next_numeric_suffix() {
        let store = tempfile::TempDir::new().unwrap();
        for (dir, repo_id) in [("my-project", "repo-1"), ("my-project-2", "repo-2")] {
            let repo_store = repo_store_path(store.path(), dir);
            let manifest = Manifest::new(repo_id, "2026-04-29T00:00:00Z");
            manifest_store::save(&repo_store, &manifest).unwrap();
        }

        let dir = allocate_repo_store_dir(store.path(), "my-project", "repo-3").unwrap();

        assert_eq!(dir, "my-project-3");
    }

    #[test]
    fn repo_store_dir_allows_existing_dir_with_same_repo_id() {
        let store = tempfile::TempDir::new().unwrap();
        let existing = store.path().join("repos/my-project");
        let manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        manifest_store::save(&existing, &manifest).unwrap();

        let dir = allocate_repo_store_dir(store.path(), "my-project", "repo-1").unwrap();

        assert_eq!(dir, "my-project");
    }
}
