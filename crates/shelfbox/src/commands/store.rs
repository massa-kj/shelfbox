use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;
use clap::Subcommand;
use shelfbox_core::api;

// ── store subcommands ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum StoreCommand {
    /// Show store metadata (path, repo count, disk usage).
    Info,

    /// Run a deep integrity check across all store contents.
    ///
    /// Exit codes: 0 = all OK, 2 = issues found.
    Verify,

    /// Delete explicitly orphaned store items.
    Gc {
        /// Print what would be deleted without making any changes.
        #[arg(long)]
        dry_run: bool,

        /// Skip confirmation prompt and perform deletions immediately.
        #[arg(long)]
        yes: bool,
    },

    /// Rebuild index.json from repository manifests.
    RebuildIndex {
        /// Print what would be indexed without writing index.json.
        #[arg(long)]
        dry_run: bool,
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
        StoreCommand::RebuildIndex { dry_run } => {
            cmd_store_rebuild_index(store_override, dry_run)?;
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
    let cfg = api::store::load_config(store_override)?;
    let idx = api::store::load_index(&cfg.store)?;
    let store_meta = api::store::load_store_meta(&cfg.store)?;

    let mut total_items: usize = 0;
    let mut repo_count: usize = 0;
    for (_id, entry) in idx.iter() {
        repo_count += 1;
        let repo_store = cfg.store.join("repos").join(&entry.repo_store_dir);
        if let Ok(mf) = api::store::load_manifest(&repo_store) {
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
    let cfg = api::store::load_config(store_override)?;
    let idx = api::store::load_index(&cfg.store)?;

    let mut issues: usize = 0;

    for (_id, entry) in idx.iter() {
        let repo_store = cfg.store.join("repos").join(&entry.repo_store_dir);
        let mf = match api::store::load_manifest(&repo_store) {
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
    let cfg = api::store::load_config(store_override)?;
    let plan = api::store::gc_plan(&cfg.store)?;

    if plan.candidates.is_empty() {
        println!("No orphaned items found.");
        print_gc_protected_summary(&plan);
        return Ok(());
    }

    print_gc_plan(&plan);

    if dry_run {
        println!("Dry run - no changes made.");
        return Ok(());
    }

    if !yes && !confirm_store_gc(plan.candidates.len(), planned_bytes(&plan))? {
        println!("No changes made.");
        return Ok(());
    }

    let report = api::store::gc_run(&cfg.store, false)?;

    println!(
        "Deleted {} orphaned item(s), {} already missing, reclaimed {}.",
        report.deleted_items,
        report.missing_items,
        human_bytes(report.bytes_reclaimed)
    );
    println!("Updated {} manifest(s).", report.manifests_updated);

    Ok(())
}

fn cmd_store_rebuild_index(store_override: Option<&Path>, dry_run: bool) -> Result<()> {
    let cfg = api::store::load_config(store_override)?;
    let report = api::store::rebuild_index(&cfg.store, dry_run)?;

    for warning in &report.warnings {
        eprintln!("Warning: {}", warning.message);
    }

    let warning_count = report.warnings.len();
    let noun = if warning_count == 1 {
        "warning"
    } else {
        "warnings"
    };

    if dry_run {
        println!("Dry run - no index written.");
        println!(
            "Would rebuild index: {} repositories, {} {}",
            report.repositories, warning_count, noun
        );
    } else if warning_count == 0 {
        println!(
            "Rebuilt index: {} repositories, 0 errors",
            report.repositories
        );
    } else {
        println!(
            "Rebuilt index: {} repositories, {} {}",
            report.repositories, warning_count, noun
        );
    }

    Ok(())
}

fn cmd_store_migrate_manifests(store_override: Option<&Path>, dry_run: bool) -> Result<()> {
    let cfg = api::store::load_config(store_override)?;
    let report = api::store::migrate_manifests(&cfg.store, dry_run)?;

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

fn print_gc_plan(plan: &api::store::GcPlan) {
    println!("Orphaned items eligible for deletion:");
    for item in &plan.candidates {
        let missing = if item.store_exists { "" } else { " (missing)" };
        println!(
            "  repos/{}/{} [{}] - {}{}",
            item.repo_store_dir,
            item.store_path,
            item.repo_id,
            human_bytes(item.size_bytes),
            missing
        );
    }
    println!(
        "Total: {} item(s), {}.",
        plan.candidates.len(),
        human_bytes(planned_bytes(plan))
    );
    print_gc_protected_summary(plan);
}

fn print_gc_protected_summary(plan: &api::store::GcPlan) {
    let protected = plan.protected_attached + plan.protected_detached + plan.protected_unreachable;
    if protected == 0 {
        return;
    }

    println!(
        "Protected: {} attached, {} detached, {} unreachable.",
        plan.protected_attached, plan.protected_detached, plan.protected_unreachable
    );
}

fn planned_bytes(plan: &api::store::GcPlan) -> u64 {
    plan.candidates.iter().map(|item| item.size_bytes).sum()
}

fn confirm_store_gc(item_count: usize, bytes: u64) -> Result<bool> {
    print!(
        "Delete {item_count} orphaned item(s), reclaiming {}? [y/N] ",
        human_bytes(bytes)
    );
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let answer = input.trim();

    Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}
