use std::{collections::HashMap, path::PathBuf};

use crate::{
    error::Result,
    policy::migration_policy::{self, ManifestVersionDecision},
    store::{
        manifest::{self, Manifest},
        manifest_legacy::{self, LegacyConversionStats, V2Manifest},
    },
};

pub use crate::plan::manifest_migration::{MigrationReport, MigrationSkip};

struct PlannedWrite {
    repo_store: PathBuf,
    manifest: Manifest,
}

/// Migrates legacy manifests under `<store>/repos/*/manifest.json` to the
/// current manifest schema.
///
/// The operation is explicit; normal manifest loading never performs migration.
/// Duplicate repository or item identities abort the whole operation before any
/// manifest is written.
pub fn run(store_root: &std::path::Path, dry_run: bool) -> Result<MigrationReport> {
    let repos_dir = store_root.join("repos");
    let mut report = MigrationReport {
        dry_run,
        target_version: Manifest::CURRENT_VERSION,
        ..MigrationReport::default()
    };

    let Ok(entries) = std::fs::read_dir(&repos_dir) else {
        return Ok(report);
    };

    let mut repo_stores: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.is_dir() && manifest::manifest_path(path).is_file())
        .collect();
    repo_stores.sort();

    let mut seen_repo_ids: HashMap<String, String> = HashMap::new();
    let mut seen_item_ids: HashMap<String, String> = HashMap::new();
    let mut planned_writes = Vec::new();

    for repo_store in repo_stores {
        let repo_store_dir = repo_store
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| repo_store.display().to_string());

        let version = match manifest::read_version(&repo_store) {
            Ok(version) => version,
            Err(err) => {
                report.skipped.push(MigrationSkip {
                    repo_store_dir,
                    reason: err.to_string(),
                });
                continue;
            }
        };

        match migration_policy::decide_manifest_version(
            version,
            V2Manifest::VERSION,
            Manifest::CURRENT_VERSION,
        ) {
            ManifestVersionDecision::ConvertLegacy => {
                let legacy = match manifest_legacy::load_v2(&repo_store) {
                    Ok(legacy) => legacy,
                    Err(err) => {
                        report.skipped.push(MigrationSkip {
                            repo_store_dir,
                            reason: err.to_string(),
                        });
                        continue;
                    }
                };

                migration_policy::record_unique_id(
                    &mut seen_repo_ids,
                    "repo_id",
                    legacy.repo_id(),
                    &repo_store_dir,
                )?;
                for item_id in legacy.item_ids() {
                    migration_policy::record_unique_id(
                        &mut seen_item_ids,
                        "item_id",
                        item_id,
                        &repo_store_dir,
                    )?;
                }

                let (converted, stats) = legacy.into_current();
                add_converted(&mut report, V2Manifest::VERSION, &stats);
                planned_writes.push(PlannedWrite {
                    repo_store,
                    manifest: converted,
                });
            }
            ManifestVersionDecision::AlreadyCurrent => {
                let current = match manifest::load(&repo_store) {
                    Ok(current) => current,
                    Err(err) => {
                        report.skipped.push(MigrationSkip {
                            repo_store_dir,
                            reason: err.to_string(),
                        });
                        continue;
                    }
                };

                migration_policy::record_unique_id(
                    &mut seen_repo_ids,
                    "repo_id",
                    &current.repo_id,
                    &repo_store_dir,
                )?;
                for item in &current.items {
                    migration_policy::record_unique_id(
                        &mut seen_item_ids,
                        "item_id",
                        &item.item_id,
                        &repo_store_dir,
                    )?;
                }

                add_unchanged(&mut report, Manifest::CURRENT_VERSION);
            }
            ManifestVersionDecision::SkipUnsupported => report.skipped.push(MigrationSkip {
                repo_store_dir,
                reason: migration_policy::unsupported_version_message(
                    version,
                    V2Manifest::VERSION,
                    Manifest::CURRENT_VERSION,
                ),
            }),
        }
    }

    if !dry_run {
        for planned in planned_writes {
            manifest::save(&planned.repo_store, &planned.manifest)?;
        }
    }

    Ok(report)
}

fn add_converted(report: &mut MigrationReport, source_version: u32, stats: &LegacyConversionStats) {
    *report.converted.entry(source_version).or_default() += 1;
    report.stale_to_unreachable += stats.stale_to_unreachable;
    report.adopted_to_detached += stats.adopted_to_detached;
    report.namespace_entries_dropped += stats.namespace_entries_dropped;
}

fn add_unchanged(report: &mut MigrationReport, version: u32) {
    *report.unchanged.entry(version).or_default() += 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::manifest::{manifest_path, OwnershipState};
    use tempfile::TempDir;

    fn write_manifest(store: &std::path::Path, dir: &str, json: &str) -> PathBuf {
        let repo_store = store.join("repos").join(dir);
        std::fs::create_dir_all(&repo_store).unwrap();
        std::fs::write(manifest_path(&repo_store), json).unwrap();
        repo_store
    }

    fn legacy_manifest(repo_id: &str, item_id: &str, state: &str) -> String {
        format!(
            r#"{{
              "version": 2,
              "repo": {{
                "id": "{repo_id}",
                "name": "my-project",
                "remote": "git@github.com:example/my-project.git"
              }},
              "items": [{{
                "item_id": "{item_id}",
                "origin_repo_id": "{repo_id}",
                "path": ".env",
                "store_path": "items/.env",
                "kind": "file",
                "link": {{"type": "symlink"}},
                "git": {{"was_tracked": false}},
                "ownership_state": "{state}",
                "created_at": "2026-04-29T00:00:00Z",
                "updated_at": "2026-04-30T00:00:00Z"
              }}],
              "namespaces": [{{
                "path": "secrets/",
                "created_at": "2026-04-29T00:00:00Z",
                "updated_at": "2026-04-29T00:00:00Z"
              }}]
            }}"#
        )
    }

    #[test]
    fn migrates_v2_manifest_to_v3() {
        let dir = TempDir::new().unwrap();
        let repo_id = "01JWPQ3VKGE93V9BDHAENVXFA5";
        let item_id = "01JWPQ3VKGE93V9BDHAENVXFA6";
        let repo_store = write_manifest(
            dir.path(),
            "my-project",
            &legacy_manifest(repo_id, item_id, "attached"),
        );

        let report = run(dir.path(), false).unwrap();
        assert_eq!(report.converted_total(), 1);
        assert_eq!(report.converted.get(&2), Some(&1));
        assert_eq!(report.unchanged_total(), 0);
        assert_eq!(report.namespace_entries_dropped, 1);

        let migrated = manifest::load(&repo_store).unwrap();
        assert_eq!(migrated.repo_id, repo_id);
        assert_eq!(migrated.items[0].item_id, item_id);
        assert_eq!(migrated.items[0].path, ".env");
        assert_eq!(migrated.items[0].store_path, "items/.env");
        assert_eq!(migrated.items[0].ownership_state, OwnershipState::Attached);
        assert_eq!(
            migrated.identity_hints.remote_hints,
            vec!["github.com/example/my-project"]
        );
        assert_eq!(migrated.identity_hints.repo_name_hints, vec!["my-project"]);
    }

    #[test]
    fn maps_legacy_states() {
        let dir = TempDir::new().unwrap();
        let stale_repo = write_manifest(
            dir.path(),
            "stale-repo",
            &legacy_manifest(
                "01JWPQ3VKGE93V9BDHAENVXFA1",
                "01JWPQ3VKGE93V9BDHAENVXFA2",
                "stale",
            ),
        );
        let adopted_repo = write_manifest(
            dir.path(),
            "adopted-repo",
            &legacy_manifest(
                "01JWPQ3VKGE93V9BDHAENVXFA3",
                "01JWPQ3VKGE93V9BDHAENVXFA4",
                "adopted",
            ),
        );

        let report = run(dir.path(), false).unwrap();
        assert_eq!(report.stale_to_unreachable, 1);
        assert_eq!(report.adopted_to_detached, 1);
        assert_eq!(
            manifest::load(&stale_repo).unwrap().items[0].ownership_state,
            OwnershipState::Unreachable
        );
        assert_eq!(
            manifest::load(&adopted_repo).unwrap().items[0].ownership_state,
            OwnershipState::Detached
        );
    }

    #[test]
    fn dry_run_reports_without_writing() {
        let dir = TempDir::new().unwrap();
        let repo_store = write_manifest(
            dir.path(),
            "my-project",
            &legacy_manifest(
                "01JWPQ3VKGE93V9BDHAENVXFA5",
                "01JWPQ3VKGE93V9BDHAENVXFA6",
                "stale",
            ),
        );

        let report = run(dir.path(), true).unwrap();
        assert!(report.dry_run);
        assert_eq!(report.converted_total(), 1);
        assert_eq!(report.stale_to_unreachable, 1);
        assert_eq!(manifest::read_version(&repo_store).unwrap(), 2);
    }

    #[test]
    fn duplicate_id_aborts_before_writing() {
        let dir = TempDir::new().unwrap();
        let first = write_manifest(
            dir.path(),
            "a",
            &legacy_manifest(
                "01JWPQ3VKGE93V9BDHAENVXFA5",
                "01JWPQ3VKGE93V9BDHAENVXFA6",
                "attached",
            ),
        );
        write_manifest(
            dir.path(),
            "b",
            &legacy_manifest(
                "01JWPQ3VKGE93V9BDHAENVXFA5",
                "01JWPQ3VKGE93V9BDHAENVXFA7",
                "attached",
            ),
        );

        let err = run(dir.path(), false).unwrap_err();
        assert!(err.to_string().contains("duplicate repo_id"));
        assert_eq!(manifest::read_version(&first).unwrap(), 2);
    }
}
