use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Subcommand;
use serde::Serialize;
use shelfbox_core::{
    config::Config,
    context,
    ignore::GitInfoExclude,
    link::DefaultLinkStrategy,
    ops,
    ops::integrity::IntegrityReport,
    store::{index, manifest, manifest::OwnershipState},
};

use crate::cmd::format::OutputFormat;

// ── repo subcommands ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
pub enum RepoCommand {
    /// List all repositories known to the store.
    List {
        /// Output format.
        #[arg(long, value_enum)]
        format: Option<OutputFormat>,
        /// Show extended fields for each repository.
        #[arg(long)]
        verbose: bool,
    },

    /// Show the health status of the current repository's shelf.
    ///
    /// Exit codes: 0 = all OK, 1 = warnings only, 2 = errors present.
    Status {
        /// Output format.
        #[arg(long, value_enum)]
        format: Option<OutputFormat>,
        /// Show extended fields for each item.
        #[arg(long)]
        verbose: bool,
    },

    /// Apply safe automatic repairs (broken symlinks, exclude, root mismatch).
    Repair {
        /// Print what would happen without making any changes.
        #[arg(long)]
        dry_run: bool,

        /// Replace wrong-target symlinks instead of reporting them as failures.
        #[arg(long)]
        force: bool,
    },

    /// Inspect current-repository store files not referenced by the manifest.
    Gc {
        /// Print inspection output without making any changes.
        #[arg(long)]
        dry_run: bool,

        /// Deprecated; repo gc is inspection-only. Use `store gc --yes`.
        #[arg(long)]
        yes: bool,
    },

    /// Associate the current clone with an existing RepoId.
    Reclaim {
        /// Reclaim this repository identity directly, skipping selection.
        #[arg(long)]
        repo_id: Option<String>,
    },

    /// Migrate the manifest schema to the current version (not yet implemented).
    #[command(hide = true)]
    Migrate,
}

// ── repo command runner ─────────────────────────────────────────────────────────────────────────

pub fn run_repo(
    command: RepoCommand,
    cwd: &Path,
    store_override: Option<&Path>,
) -> Result<ExitCode> {
    match command {
        RepoCommand::List { format, verbose } => {
            cmd_repo_list(store_override, format, verbose)?;
            Ok(ExitCode::SUCCESS)
        }
        RepoCommand::Status { format, verbose } => {
            cmd_repo_status(cwd, store_override, format, verbose)
        }
        RepoCommand::Repair { dry_run, force } => {
            cmd_repo_repair(cwd, store_override, dry_run, force)?;
            Ok(ExitCode::SUCCESS)
        }
        RepoCommand::Gc { dry_run, yes } => {
            cmd_repo_gc(cwd, store_override, dry_run, yes)?;
            Ok(ExitCode::SUCCESS)
        }
        RepoCommand::Reclaim { repo_id } => {
            cmd_repo_reclaim(cwd, store_override, repo_id.as_deref())?;
            Ok(ExitCode::SUCCESS)
        }
        RepoCommand::Migrate => {
            anyhow::bail!("not yet implemented")
        }
    }
}

// ── Subcommand handlers ─────────────────────────────────────────────────────────────────────────

/// Snapshot of a single repository entry for list output.
#[derive(Debug, Serialize)]
struct RepoSummary {
    name: String,
    root: Option<PathBuf>,
    item_count: usize,
    last_seen_at: String,
}

fn repo_entry_name(entry: &index::RepoEntry) -> String {
    entry
        .root
        .as_ref()
        .and_then(|root| root.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| entry.repo_store_dir.clone())
}

fn repo_root_label(root: Option<&PathBuf>) -> String {
    root.map(|root| root.display().to_string())
        .unwrap_or_else(|| "(unassociated)".to_string())
}

fn cmd_repo_list(
    store_override: Option<&Path>,
    format: Option<OutputFormat>,
    verbose: bool,
) -> Result<()> {
    let config = Config::load(store_override).context("failed to load config")?;
    let idx = index::load(&config.store).context("failed to load store index")?;

    let mut rows: Vec<RepoSummary> = idx
        .iter()
        .map(|(_id, entry)| {
            let repo_store = config.store.join("repos").join(&entry.repo_store_dir);
            let item_count = manifest::load(&repo_store)
                .map(|m| m.items.len())
                .unwrap_or(0);
            RepoSummary {
                name: repo_entry_name(entry),
                root: entry.root.clone(),
                item_count,
                last_seen_at: entry.last_seen_at.clone(),
            }
        })
        .collect();

    // Stable sort by repository name for deterministic output.
    rows.sort_by(|a, b| a.name.cmp(&b.name));

    let fmt = OutputFormat::resolve(format, &config.default_format);

    match fmt {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&rows)?),
        OutputFormat::Plain => {
            for r in &rows {
                println!(
                    "{} {} {}",
                    r.name,
                    repo_root_label(r.root.as_ref()),
                    r.item_count
                );
            }
        }
        OutputFormat::Table => {
            if verbose {
                // Verbose: one multi-line block per repository.
                let mut entries: Vec<(&str, &_)> = idx.iter().collect();
                entries.sort_by(|(_, a), (_, b)| {
                    let na = a
                        .root
                        .as_ref()
                        .and_then(|root| root.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| a.repo_store_dir.clone());
                    let nb = b
                        .root
                        .as_ref()
                        .and_then(|root| root.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| b.repo_store_dir.clone());
                    na.cmp(&nb)
                });
                if entries.is_empty() {
                    println!("(no repositories in store)");
                    return Ok(());
                }
                for (_, entry) in &entries {
                    let repo_store = config.store.join("repos").join(&entry.repo_store_dir);
                    let item_count = manifest::load(&repo_store)
                        .map(|m| m.items.len())
                        .unwrap_or(0);
                    let name = repo_entry_name(entry);
                    println!("  {name}");
                    println!("    root:        {}", repo_root_label(entry.root.as_ref()));
                    println!(
                        "    git_common:  {}",
                        entry
                            .git_common_dir
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "(unassociated)".to_string())
                    );
                    println!("    store_dir:   {}", entry.repo_store_dir);
                    println!("    items:       {item_count}");
                    println!("    last_seen:   {}", entry.last_seen_at);
                    println!();
                }
            } else {
                if rows.is_empty() {
                    println!("(no repositories in store)");
                    return Ok(());
                }
                println!("  {:<30} {:<50} {:>5}  last seen", "name", "root", "items");
                println!("  {}", "-".repeat(100));
                for r in &rows {
                    println!(
                        "  {:<30} {:<50} {:>5}  {}",
                        r.name,
                        repo_root_label(r.root.as_ref()),
                        r.item_count,
                        r.last_seen_at,
                    );
                }
            }
        }
    }
    Ok(())
}

fn cmd_repo_status(
    cwd: &Path,
    store_override: Option<&Path>,
    format: Option<OutputFormat>,
    verbose: bool,
) -> Result<ExitCode> {
    let ctx =
        context::build(cwd, store_override, false).context("failed to initialise repo context")?;
    let fmt = OutputFormat::resolve(format, &ctx.config.default_format);
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;
    let report = ops::integrity::check(&ctx, &link, &ignore)?;

    // Read-only scan: surface pending ownership transitions without writing.
    // Actual transitions happen in `repo repair` to keep status side-effect-free.
    let pending = ops::detect_transitions::scan(&ctx, &ctx.config.clone()).unwrap_or_default();
    if !pending.is_empty() {
        eprintln!(
            "hint: {} item(s) in {} repo(s) may need ownership transition \
             (unreachable: {}) — run 'shelfbox repo repair' to apply",
            pending.unreachable,
            pending.affected_repos.len(),
            pending.unreachable,
        );
    }

    match fmt {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&report)?),
        OutputFormat::Plain => print_repo_status_plain(&report),
        OutputFormat::Table => print_repo_status(&report, verbose, &ctx.repo_root),
    }
    Ok(classify_integrity_exit(&report))
}

fn cmd_repo_repair(
    cwd: &Path,
    store_override: Option<&Path>,
    dry_run: bool,
    force: bool,
) -> Result<()> {
    let config = Config::load(store_override).context("failed to load config")?;
    let current =
        context::current_git_context(cwd).context("failed to inspect current git repository")?;
    let idx = index::load(&config.store).context("failed to load store index")?;
    let associated_repo_id = context::resolve_existing_repo(&current, &idx)
        .ok_or_else(|| anyhow::anyhow!("Run `shelfbox repo reclaim` first"))?;

    let mut ctx =
        context::build(cwd, store_override, true).context("failed to initialise repo context")?;
    let link = DefaultLinkStrategy;
    if ctx.repo_id != associated_repo_id {
        anyhow::bail!("Run `shelfbox repo reclaim` first");
    }

    let report = ops::repair::repair_repo(&mut ctx, &link, dry_run, force)?;
    print_repo_repair_report(&report, dry_run);
    Ok(())
}

fn cmd_repo_gc(cwd: &Path, store_override: Option<&Path>, dry_run: bool, yes: bool) -> Result<()> {
    let ctx =
        context::build(cwd, store_override, false).context("failed to initialise repo context")?;
    let link = DefaultLinkStrategy;
    let ignore = GitInfoExclude;

    // Use check() to inspect unreferenced store files without deleting them.
    let report = ops::integrity::check(&ctx, &link, &ignore)?;

    // Inform the user about items protected from GC by their ownership state.
    // These are in the manifest and will never appear as FS orphans, but it is
    // useful to surface them so the user knows what `gc` is not collecting.
    let detached_count = ctx
        .manifest
        .items
        .iter()
        .filter(|i| i.ownership_state == OwnershipState::Detached)
        .count();
    let unreachable_count = ctx
        .manifest
        .items
        .iter()
        .filter(|i| i.ownership_state == OwnershipState::Unreachable)
        .count();

    if detached_count > 0 || unreachable_count > 0 {
        println!("ownership-protected items (not collected by gc):");
        if detached_count > 0 {
            println!(
                "  {detached_count} detached — run 'shelfbox item relink <PATH>' to re-attach"
            );
        }
        if unreachable_count > 0 {
            println!("  {unreachable_count} unreachable — run 'shelfbox repo reclaim' then 'shelfbox repo repair' to recover");
        }
    }

    if report.orphan_store_items.is_empty() {
        println!("no unreferenced current-repository store files found");
        return Ok(());
    }

    if dry_run {
        println!("unreferenced current-repository store files:");
    } else {
        println!("unreferenced current-repository store files (not deleted):");
    }
    for orphan in &report.orphan_store_items {
        println!("  {orphan}");
    }
    if yes {
        println!("note: --yes is ignored by repo gc; use `shelfbox store gc --yes` for confirmed conservative deletion of manifest items marked orphaned");
    } else {
        println!("note: repo gc is inspection-only; use `shelfbox store gc` for conservative deletion of manifest items marked orphaned");
    }

    Ok(())
}

fn cmd_repo_reclaim(
    cwd: &Path,
    store_override: Option<&Path>,
    requested_repo_id: Option<&str>,
) -> Result<()> {
    let config = Config::load(store_override).context("failed to load config")?;
    let current =
        context::current_git_context(cwd).context("failed to inspect current git repository")?;
    let idx = index::load(&config.store).context("failed to load store index")?;

    let current_manifest = match context::resolve_existing_repo(&current, &idx) {
        Some(repo_id) => {
            let entry = idx
                .get(&repo_id)
                .with_context(|| format!("index entry disappeared for repo_id {repo_id}"))?;
            let repo_store = config.store.join("repos").join(&entry.repo_store_dir);
            Some(
                manifest::load(&repo_store)
                    .with_context(|| format!("failed to load current manifest for {repo_id}"))?,
            )
        }
        None => None,
    };
    ops::reclaim::check_reclaim_precondition(current_manifest.as_ref())?;

    let repo_id = if let Some(repo_id) = requested_repo_id {
        repo_id.to_string()
    } else {
        let candidates = ops::reclaim::build_candidates(
            &config.store,
            &current.repo_root,
            current.remote_hint.as_deref(),
            &idx,
        )?;

        if candidates.is_empty() {
            println!("No reclaim candidates found.");
            return Ok(());
        }

        print_reclaim_candidates(&candidates);
        match prompt_reclaim_selection(candidates.len())? {
            Some(index) => candidates[index].repo_id.clone(),
            None => {
                println!("No changes made.");
                return Ok(());
            }
        }
    };

    let outcome = ops::reclaim::execute_reclaim(&config.store, &current, &repo_id)?;
    println!(
        "Associated with {}. Run `shelfbox repo repair` to restore symlinks.",
        outcome.repo_id
    );

    Ok(())
}

// ── Human-readable formatters ───────────────────────────────────────────────────────────────────

fn print_reclaim_candidates(candidates: &[ops::reclaim::ReclaimCandidate]) {
    println!("Reclaim candidates:");
    println!();

    for (idx, candidate) in candidates.iter().enumerate() {
        println!("{}. {}", idx + 1, candidate.repo_store_dir);
        println!("   repo_id: {}", candidate.repo_id);
        println!("   score:   {}", candidate.score);
        println!(
            "   reason:  {}",
            if candidate.reasons.is_empty() {
                "(none)".to_string()
            } else {
                candidate.reasons.join(", ")
            }
        );
        println!("   items:   {}", candidate.item_count);
        println!("   state:   {}", candidate_state_label(candidate.state));
        println!(
            "   remote:  {}",
            if candidate.remote_hints.is_empty() {
                "(none)".to_string()
            } else {
                candidate.remote_hints.join(", ")
            }
        );
        println!(
            "   names:   {}",
            if candidate.repo_name_hints.is_empty() {
                "(none)".to_string()
            } else {
                candidate.repo_name_hints.join(", ")
            }
        );
        println!();
    }
}

fn prompt_reclaim_selection(candidate_count: usize) -> Result<Option<usize>> {
    print!("Select [1-{candidate_count}] or q to quit: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();

    if input.eq_ignore_ascii_case("q") {
        return Ok(None);
    }

    let selected: usize = input
        .parse()
        .with_context(|| format!("invalid selection '{input}'"))?;
    if selected == 0 || selected > candidate_count {
        anyhow::bail!("selection out of range: {selected}");
    }

    Ok(Some(selected - 1))
}

fn candidate_state_label(state: ops::reclaim::CandidateState) -> &'static str {
    match state {
        ops::reclaim::CandidateState::Unreachable => "unreachable",
        ops::reclaim::CandidateState::Detached => "detached",
        ops::reclaim::CandidateState::AttachedElsewhere => "attached elsewhere",
    }
}

fn print_repo_repair_report(report: &ops::repair::RepairRepoReport, dry_run: bool) {
    let repaired_label = if dry_run { "would repair" } else { "repaired" };

    println!("repo repair:");
    println!("  symlinks {repaired_label}: {}", report.symlinks_repaired);
    println!(
        "  symlinks already healthy: {}",
        report.symlinks_already_healthy
    );
    println!("  symlinks failed: {}", report.symlinks_failed.len());
    for (path, reason) in &report.symlinks_failed {
        eprintln!("    failed {path}: {reason}");
    }
    println!(
        "  exclude: {}",
        repair_change_label(report.exclude_updated, dry_run)
    );
    println!(
        "  index: {}",
        repair_change_label(report.index_updated, dry_run)
    );
    println!(
        "  identity hints: {}",
        repair_change_label(report.hints_updated, dry_run)
    );
}

fn repair_change_label(changed: bool, dry_run: bool) -> &'static str {
    match (changed, dry_run) {
        (true, true) => "would update",
        (true, false) => "updated",
        (false, _) => "already current",
    }
}

fn print_repo_status(report: &IntegrityReport, verbose: bool, repo_root: &Path) {
    println!("repo: {}", repo_root.display());

    let total = report.items.len();
    let errors = report.items.iter().filter(|s| !s.ok).count();
    let overall = if errors > 0 { "ERROR" } else { "OK" };

    println!("items: {total} total, {errors} with issues  [{overall}]");

    for s in &report.items {
        let label = if s.ok { "OK" } else { "ERROR" };
        if verbose {
            println!("  {label:<8} {}", s.path);
            println!("    link_exists:  {}", s.link_exists);
            println!("    link_valid:   {}", s.link_valid);
            println!("    store_exists: {}", s.store_exists);
            println!("    in_exclude:   {}", s.in_exclude);
            println!("    not_tracked:  {}", s.not_tracked);
        } else {
            let mut issues: Vec<&str> = Vec::new();
            if !s.link_exists {
                issues.push("symlink missing");
            } else if !s.link_valid {
                issues.push("symlink invalid");
            }
            if !s.store_exists {
                issues.push("store item missing");
            }
            if !s.in_exclude {
                issues.push("not in exclude");
            }
            if !s.not_tracked {
                issues.push("tracked by git");
            }
            if issues.is_empty() {
                println!("  {label:<8} {}", s.path);
            } else {
                println!("  {label:<8} {}  ({})", s.path, issues.join(", "));
            }
        }
    }

    let orphan_count = report.orphan_store_items.len();
    if orphan_count > 0 {
        println!("unreferenced store items: {orphan_count}  [WARN]");
        for o in &report.orphan_store_items {
            println!("  {o}");
        }
    } else {
        println!("unreferenced store items: 0  [OK]");
    }

    let root_label = if report.repo_root_matches_index {
        "OK"
    } else {
        "WARN"
    };
    println!("index root: [{root_label}]");
}

fn print_repo_status_plain(report: &IntegrityReport) {
    for s in &report.items {
        let label = if s.ok { "OK" } else { "ERROR" };
        println!("{label} {}", s.path);
    }
    for o in &report.orphan_store_items {
        println!("UNREFERENCED {o}");
    }
    if !report.repo_root_matches_index {
        println!("ROOT_MISMATCH");
    }
}

/// Determine the exit code for `repo status` based on the integrity report.
///
/// - 2: structural ERROR (broken/missing symlink, missing store item, git-tracked)
/// - 1: WARN only (missing exclude entry, unreferenced store items, root mismatch)
/// - 0: all clear
fn classify_integrity_exit(report: &IntegrityReport) -> ExitCode {
    let has_error = report
        .items
        .iter()
        .any(|s| !s.link_exists || !s.link_valid || !s.store_exists || !s.not_tracked);
    if has_error {
        return ExitCode::from(2);
    }

    let has_warn = report.items.iter().any(|s| !s.in_exclude)
        || !report.orphan_store_items.is_empty()
        || !report.repo_root_matches_index;
    if has_warn {
        return ExitCode::from(1);
    }

    ExitCode::SUCCESS
}
