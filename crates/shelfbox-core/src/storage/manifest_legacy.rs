use std::path::Path;

use serde::Deserialize;

use crate::{
    error::{AppError, Result},
    git,
    storage::manifest_store::{self as manifest, Item, Manifest, OwnershipState},
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyConversionStats {
    pub stale_to_unreachable: usize,
    pub adopted_to_detached: usize,
    pub namespace_entries_dropped: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct V2Manifest {
    version: u32,
    repo: V2RepoMeta,
    items: Vec<V2Item>,
    #[serde(default)]
    namespaces: Vec<V2NamespaceEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct V2RepoMeta {
    id: String,
    name: String,
    #[serde(default)]
    remote: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct V2Item {
    item_id: String,
    origin_repo_id: String,
    path: String,
    store_path: String,
    ownership_state: V2OwnershipState,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
struct V2NamespaceEntry {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum V2OwnershipState {
    Attached,
    Detached,
    Unreachable,
    Stale,
    Adopted,
    Orphaned,
}

impl V2Manifest {
    pub const VERSION: u32 = 2;

    pub fn repo_id(&self) -> &str {
        &self.repo.id
    }

    pub fn item_ids(&self) -> impl Iterator<Item = &str> {
        self.items.iter().map(|item| item.item_id.as_str())
    }

    pub fn into_current(self) -> (Manifest, LegacyConversionStats) {
        debug_assert_eq!(self.version, Self::VERSION);

        let created_at = self
            .items
            .iter()
            .map(|item| item.created_at.as_str())
            .min()
            .unwrap_or("1970-01-01T00:00:00Z")
            .to_string();
        let mut manifest = Manifest::new(self.repo.id, created_at);
        manifest.add_repo_name_hint(&self.repo.name);
        if let Some(remote) = self.repo.remote.as_deref() {
            if let Some(hint) = git::normalize_remote_hint(remote) {
                manifest.add_remote_hint(&hint);
            }
        }

        let mut stats = LegacyConversionStats {
            namespace_entries_dropped: self.namespaces.len(),
            ..LegacyConversionStats::default()
        };

        for item in self.items {
            let ownership_state = match item.ownership_state {
                V2OwnershipState::Attached => OwnershipState::Attached,
                V2OwnershipState::Detached => OwnershipState::Detached,
                V2OwnershipState::Unreachable => OwnershipState::Unreachable,
                V2OwnershipState::Stale => {
                    stats.stale_to_unreachable += 1;
                    OwnershipState::Unreachable
                }
                V2OwnershipState::Adopted => {
                    stats.adopted_to_detached += 1;
                    OwnershipState::Detached
                }
                V2OwnershipState::Orphaned => OwnershipState::Orphaned,
            };
            manifest.add(Item {
                item_id: item.item_id,
                origin_repo_id: item.origin_repo_id,
                path: item.path,
                store_path: item.store_path,
                ownership_state,
                created_at: item.created_at,
                updated_at: item.updated_at,
            });
        }

        (manifest, stats)
    }
}

pub fn load_v2(repo_store: &Path) -> Result<V2Manifest> {
    let path = manifest::manifest_path(repo_store);
    let s = std::fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))?;
    let legacy: V2Manifest =
        serde_json::from_str(&s).map_err(|e| AppError::json(path.clone(), e))?;
    if legacy.version != V2Manifest::VERSION {
        return Err(AppError::ManifestVersionMismatch {
            path,
            found: legacy.version,
            expected: V2Manifest::VERSION,
        });
    }
    Ok(legacy)
}
