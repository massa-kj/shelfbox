use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;
use clap::Subcommand;
use shelfbox_core::{
    config::Config,
    ops,
    store::{index, manifest, manifest::OwnershipState, meta},
};

// ── store subcommands ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum StoreCommand {
    /// Show store metadata (path, repo count, disk usage).
    Info,

    /// Run a deep integrity check across all store contents.
    ///
    /// Exit codes: 0 = all OK, 2 = issues found.
    Verify,

    /// Delete store entries for repositories that no longer exist.
    Gc {
        /// Print what would be deleted without making any changes.
        #[arg(long)]
        dry_run: bool,

        /// Skip confirmation prompt and perform deletions immediately.
        #[arg(long)]
        yes: bool,
    },

    /// Explicitly migrate legacy manifests to the current schema.
    MigrateManifests {
        /// Print what would be converted without writing manifests.
        #[arg(long)]
        dry_run: bool,
    },
}

// ── store command runner ────────────────────────────────────────────────────────────────────────

pub fn run_store(
    command: StoreCommand,
    _cwd: &Path,
    store_override: Option<&Path>,
) -> Result<ExitCode> {
    match command {
        StoreCommand::Info => {
            cmd_store_info(store_override)?;
            Ok(ExitCode::SUCCESS)
        }
        StoreCommand::Verify => cmd_store_verify(store_override),
        StoreCommand::Gc { dry_run, yes } => {
            cmd_store_gc(store_override, dry_run, yes)?;
            Ok(ExitCode::SUCCESS)
        }
        StoreCommand::MigrateManifests { dry_run } => {
            cmd_store_migrate_manifests(store_override, dry_run)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

// ── helpers ────────────────────────────────────────────────────────────────────────────────────

/// Compute disk usage of a directory tree in bytes.
fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| {
            let meta = e.metadata().ok();
            if let Some(m) = meta {
                if m.is_dir() {
                    dir_size(&e.path())
                } else {
                    m.len()
                }
            } else {
                0
            }
        })
        .sum()
}

/// Format bytes as a human-readable string (KiB / MiB / GiB).
fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

// ── subcommand implementations ─────────────────────────────────────────────────────────────────

fn cmd_store_info(store_override: Option<&Path>) -> Result<()> {
    let cfg = Config::load(store_override)?;
    let idx = index::load(&cfg.store)?;
    let store_meta = meta::load_store_meta(&cfg.store)?;

    let mut total_items: usize = 0;
    let mut repo_count: usize = 0;
    for (_id, entry) in idx.iter() {
        repo_count += 1;
        let repo_store = cfg.store.join("repos").join(&entry.repo_store_dir);
        if let Ok(mf) = manifest::load(&repo_store) {
            total_items += mf.items.len();
        }
    }

    let disk_bytes = dir_size(&cfg.store);

    println!("Store path  : {}", cfg.store.display());
    if let Some(ref m) = store_meta {
        println!("Store ID    : {}", m.store_id);
        let host = if m.hostname.is_empty() {
            "unknown"
        } else {
            &m.hostname
        };
        println!("Hostname    : {host}");
    }
    println!("Repositories: {repo_count}");
    println!("Total items : {total_items}");
    println!("Disk usage  : {}", human_bytes(disk_bytes));

    Ok(())
}

fn cmd_store_verify(store_override: Option<&Path>) -> Result<ExitCode> {
    let cfg = Config::load(store_override)?;
    let idx = index::load(&cfg.store)?;

    let mut issues: usize = 0;

    for (_id, entry) in idx.iter() {
        let repo_store = cfg.store.join("repos").join(&entry.repo_store_dir);
        let mf = match manifest::load(&repo_store) {
            Ok(m) => m,
            Err(e) => {
                eprintln!(
                    "WARN  repos/{} — cannot read manifest: {e}",
                    entry.repo_store_dir
                );
                issues += 1;
                continue;
            }
        };

        for item in &mf.items {
            if let Some(root) = &entry.root {
                // Check that the symlink target exists when this index entry
                // is associated with a local clone.
                let link_path = root.join(&item.path);
                if !link_path.exists() {
                    eprintln!("MISS  symlink not found: {}", link_path.display());
                    issues += 1;
                }
            }

            // Check that the store file exists.
            let store_file = repo_store.join(&item.store_path);
            if !store_file.exists() {
                eprintln!("MISS  store file not found: {}", store_file.display());
                issues += 1;
            }
        }
    }

    if issues == 0 {
        println!("OK — no issues found.");
        Ok(ExitCode::SUCCESS)
    } else {
        println!("{issues} issue(s) found. Run `shelfbox repo repair` to fix.");
        Ok(ExitCode::from(2))
    }
}

fn cmd_store_gc(store_override: Option<&Path>, dry_run: bool, yes: bool) -> Result<()> {
    let cfg = Config::load(store_override)?;
    let mut idx = index::load(&cfg.store)?;

    // Collect repos whose root directory no longer exists on disk.
    let stale: Vec<(String, std::path::PathBuf, String)> = idx
        .iter()
        .filter(|(_id, entry)| entry.root.as_ref().is_some_and(|root| !root.exists()))
        .map(|(id, entry)| {
            (
                id.to_owned(),
                entry.root.clone().expect("filtered to entries with roots"),
                entry.repo_store_dir.clone(),
            )
        })
        .collect();

    if stale.is_empty() {
        println!("Nothing to clean up.");
        return Ok(());
    }

    println!("Stale entries ({}):", stale.len());
    for (id, root, store_dir) in &stale {
        let repo_store = cfg.store.join("repos").join(store_dir);
        let reclaimable = count_reclaimable_items(&repo_store);
        if reclaimable > 0 {
            println!(
                "  {} [{}] — {} reclaimable item(s), protected from GC",
                root.display(),
                id,
                reclaimable
            );
        } else {
            println!("  {} [{}]", root.display(), id);
        }
    }

    if dry_run {
        println!("Dry run — no changes made.");
        return Ok(());
    }

    if !yes {
        println!("Run with --yes to remove eligible entries.");
        println!("note: repos with reclaimable items are always skipped — run 'shelfbox repo reclaim' from a current clone first.");
        return Ok(());
    }

    // Remove store data and index entries.
    // Repos with reclaimable items are skipped regardless of --yes (spec §7.5 / §9.2).
    let mut removed = 0;
    let mut skipped = 0;
    for (id, root, store_dir) in &stale {
        let repo_store = cfg.store.join("repos").join(store_dir);
        let reclaimable = count_reclaimable_items(&repo_store);
        if reclaimable > 0 {
            eprintln!(
                "warning: skipping '{}' [{}]: {} reclaimable item(s) \
                 — run 'shelfbox repo reclaim' from a current clone first",
                root.display(),
                id,
                reclaimable
            );
            skipped += 1;
            continue;
        }
        if repo_store.exists() {
            std::fs::remove_dir_all(&repo_store)
                .map_err(|e| anyhow::anyhow!("failed to remove {}: {e}", repo_store.display()))?;
        }
        idx.remove(id);
        println!("Removed: {}", root.display());
        removed += 1;
    }

    if removed > 0 {
        index::save(&cfg.store, &idx)?;
    }

    println!("Done. {removed} removed, {skipped} skipped (reclaimable).");

    Ok(())
}

fn cmd_store_migrate_manifests(store_override: Option<&Path>, dry_run: bool) -> Result<()> {
    let cfg = Config::load(store_override)?;
    let report = ops::migrate_manifest::run(&cfg.store, dry_run)?;

    if dry_run {
        println!("Dry run - no manifests written.");
    }
    println!("target manifest version: {}", report.target_version);
    println!("manifests converted: {}", report.converted_total());
    for (version, count) in &report.converted {
        println!("  v{} -> v{}: {}", version, report.target_version, count);
    }
    println!("manifests unchanged: {}", report.unchanged_total());
    for (version, count) in &report.unchanged {
        println!("  v{}: {}", version, count);
    }
    println!("skipped/failed: {}", report.skipped.len());
    for skipped in &report.skipped {
        println!("  {}: {}", skipped.repo_store_dir, skipped.reason);
    }
    println!(
        "ownership mappings: stale -> unreachable: {}, adopted -> detached: {}",
        report.stale_to_unreachable, report.adopted_to_detached
    );
    println!(
        "namespace entries dropped: {}",
        report.namespace_entries_dropped
    );

    Ok(())
}

/// Returns the number of items in `repo_store`'s manifest that are protected
/// from GC — i.e., in a state other than `Orphaned`.
fn count_reclaimable_items(repo_store: &std::path::Path) -> usize {
    if !repo_store.exists() {
        return 0;
    }
    manifest::load(repo_store)
        .map(|mf| {
            mf.items
                .iter()
                .filter(|i| !matches!(i.ownership_state, OwnershipState::Orphaned))
                .count()
        })
        .unwrap_or(0)
}
