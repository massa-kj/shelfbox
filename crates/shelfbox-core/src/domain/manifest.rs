use serde::{Deserialize, Serialize};

use crate::domain::ownership::OwnershipState;

/// Candidate-ranking hints. These are never proof of identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct IdentityHints {
    /// Normalized remote hints, e.g. `github.com/org/repo`.
    #[serde(default)]
    pub remote_hints: Vec<String>,
    /// Recent repository directory names, most recent first.
    #[serde(default)]
    pub repo_name_hints: Vec<String>,
    /// Last successful explicit association or repair timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_attached_at: Option<String>,
}

/// A single shelved item recorded in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// Immutable ULID assigned when this item is first shelved.
    pub item_id: String,

    /// Repository identity that originally shelved this item. Immutable.
    pub origin_repo_id: String,

    /// Path relative to the repository root (forward slashes, no leading `/`).
    pub path: String,

    /// Path of the store-side file relative to the repo's store directory.
    pub store_path: String,

    /// Current ownership state.
    pub ownership_state: OwnershipState,

    /// ISO-8601 creation timestamp.
    pub created_at: String,

    /// ISO-8601 last-updated timestamp.
    pub updated_at: String,
}

/// The in-memory representation of `manifest.json` for a single repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    version: u32,

    /// Stable repository identity.
    pub repo_id: String,

    /// ISO-8601 creation timestamp.
    pub created_at: String,

    /// Candidate-ranking and display hints.
    pub identity_hints: IdentityHints,

    /// All currently shelved items.
    pub items: Vec<Item>,
}

impl Manifest {
    pub const CURRENT_VERSION: u32 = 3;
    const MAX_REPO_NAME_HINTS: usize = 5;

    /// Creates a new, empty manifest for the given repository.
    pub fn new(repo_id: impl Into<String>, created_at: impl Into<String>) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            repo_id: repo_id.into(),
            created_at: created_at.into(),
            identity_hints: IdentityHints::default(),
            items: Vec::new(),
        }
    }

    /// Returns the item whose repo-relative path matches `path`, if any.
    pub fn get(&self, path: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.path == path)
    }

    /// Returns `true` if `path` is already recorded in this manifest.
    pub fn contains(&self, path: &str) -> bool {
        self.get(path).is_some()
    }

    /// Appends a new item. Panics in debug builds if `path` already exists.
    pub fn add(&mut self, item: Item) {
        debug_assert!(
            !self.contains(&item.path),
            "item '{}' already in manifest",
            item.path
        );
        self.items.push(item);
    }

    /// Removes the item with the given `path`. Returns `true` if removed.
    pub fn remove(&mut self, path: &str) -> bool {
        let before = self.items.len();
        self.items.retain(|i| i.path != path);
        self.items.len() < before
    }

    /// Sets the ownership state of the item at `path`, updating `updated_at`.
    pub fn set_ownership_state(
        &mut self,
        path: &str,
        state: OwnershipState,
        updated_at: &str,
    ) -> bool {
        if let Some(item) = self.items.iter_mut().find(|i| i.path == path) {
            item.ownership_state = state;
            item.updated_at = updated_at.to_string();
            true
        } else {
            false
        }
    }

    /// Renames a manifest item in-place.
    pub fn rename(
        &mut self,
        old_path: &str,
        new_path: &str,
        new_store_path: &str,
        updated_at: &str,
    ) -> bool {
        if let Some(item) = self.items.iter_mut().find(|i| i.path == old_path) {
            item.path = new_path.to_string();
            item.store_path = new_store_path.to_string();
            item.updated_at = updated_at.to_string();
            true
        } else {
            false
        }
    }

    /// Adds a normalized remote hint if not already present.
    pub fn add_remote_hint(&mut self, hint: &str) {
        if hint.is_empty() {
            return;
        }
        if !self.identity_hints.remote_hints.iter().any(|h| h == hint) {
            self.identity_hints.remote_hints.push(hint.to_string());
        }
    }

    /// Adds a repository-name hint, most recent first.
    pub fn add_repo_name_hint(&mut self, name: &str) {
        if name.is_empty() {
            return;
        }
        self.identity_hints.repo_name_hints.retain(|h| h != name);
        self.identity_hints
            .repo_name_hints
            .insert(0, name.to_string());
        self.identity_hints
            .repo_name_hints
            .truncate(Self::MAX_REPO_NAME_HINTS);
    }

    /// Updates `last_attached_at`.
    pub fn touch_attached_at(&mut self, now: impl Into<String>) {
        self.identity_hints.last_attached_at = Some(now.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_manifest() -> Manifest {
        Manifest::new("01JWPQ3VKGE93V9BDHAENVXFA5", "2026-04-29T00:00:00Z")
    }

    fn sample_item(path: &str) -> Item {
        Item {
            item_id: "01JWPQ3VKGE93V9BDHAENVXFA6".into(),
            origin_repo_id: "01JWPQ3VKGE93V9BDHAENVXFA5".into(),
            path: path.to_string(),
            store_path: format!("items/{path}"),
            ownership_state: OwnershipState::Attached,
            created_at: "2026-04-29T00:00:00Z".into(),
            updated_at: "2026-04-29T00:00:00Z".into(),
        }
    }

    #[test]
    fn identity_hints_omits_absent_last_attached_at() {
        let hints = IdentityHints {
            remote_hints: vec!["github.com/example/app".into()],
            repo_name_hints: vec!["app".into()],
            last_attached_at: None,
        };

        assert_eq!(
            serde_json::to_value(hints).unwrap(),
            json!({
                "remote_hints": ["github.com/example/app"],
                "repo_name_hints": ["app"]
            })
        );
    }

    #[test]
    fn identity_hints_defaults_missing_collections() {
        let hints: IdentityHints = serde_json::from_value(json!({})).unwrap();

        assert!(hints.remote_hints.is_empty());
        assert!(hints.repo_name_hints.is_empty());
        assert_eq!(hints.last_attached_at, None);
    }

    #[test]
    fn manifest_json_shape_matches_v3_schema() {
        let mut manifest = sample_manifest();
        manifest.add_remote_hint("github.com/example/app");
        manifest.add_repo_name_hint("app");
        manifest.touch_attached_at("2026-05-01T00:00:00Z");
        manifest.add(sample_item("secrets.env"));

        assert_eq!(
            serde_json::to_value(manifest).unwrap(),
            json!({
                "version": 3,
                "repo_id": "01JWPQ3VKGE93V9BDHAENVXFA5",
                "created_at": "2026-04-29T00:00:00Z",
                "identity_hints": {
                    "remote_hints": ["github.com/example/app"],
                    "repo_name_hints": ["app"],
                    "last_attached_at": "2026-05-01T00:00:00Z"
                },
                "items": [{
                    "item_id": "01JWPQ3VKGE93V9BDHAENVXFA6",
                    "origin_repo_id": "01JWPQ3VKGE93V9BDHAENVXFA5",
                    "path": "secrets.env",
                    "store_path": "items/secrets.env",
                    "ownership_state": "attached",
                    "created_at": "2026-04-29T00:00:00Z",
                    "updated_at": "2026-04-29T00:00:00Z"
                }]
            })
        );
    }

    #[test]
    fn manifest_methods_update_items_and_hints_without_reordering_remotes() {
        let mut manifest = sample_manifest();
        manifest.add_remote_hint("github.com/example/a");
        manifest.add_remote_hint("github.com/example/b");
        manifest.add_remote_hint("github.com/example/a");
        for name in ["a", "b", "c", "d", "e", "f"] {
            manifest.add_repo_name_hint(name);
        }
        manifest.add_repo_name_hint("d");
        manifest.add(sample_item("old.env"));

        assert_eq!(
            manifest.identity_hints.remote_hints,
            vec!["github.com/example/a", "github.com/example/b"]
        );
        assert_eq!(
            manifest.identity_hints.repo_name_hints,
            vec!["d", "f", "e", "c", "b"]
        );
        assert!(manifest.rename(
            "old.env",
            "new.env",
            "items/new.env",
            "2026-05-25T00:00:00Z"
        ));
        assert!(!manifest.contains("old.env"));
        assert_eq!(manifest.get("new.env").unwrap().store_path, "items/new.env");
        assert!(manifest.set_ownership_state(
            "new.env",
            OwnershipState::Detached,
            "2026-05-26T00:00:00Z"
        ));
        assert_eq!(
            manifest.get("new.env").unwrap().ownership_state,
            OwnershipState::Detached
        );
        assert!(manifest.remove("new.env"));
        assert!(manifest.items.is_empty());
    }
}
