use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};

use fd_lock::{RwLock as FdRwLock, RwLockReadGuard, RwLockWriteGuard};
use ulid::Ulid;

use crate::{
    config::Config,
    error::{AppError, Result},
    store::{
        index::{self, Index, RepoEntry},
        manifest::{self, Manifest},
        meta,
    },
};

// ── Store-level advisory lock ─────────────────────────────────────────────────

/// Guard variant held inside [`StoreLock`].
enum StoreLockGuard {
    Write(ManuallyDrop<RwLockWriteGuard<'static, std::fs::File>>),
    Read(ManuallyDrop<RwLockReadGuard<'static, std::fs::File>>),
}

/// Advisory file lock held on `<store>/.lock` for the duration of a
/// [`RepoContext`] lifetime.
///
/// Prevents concurrent write–write and write–read conflicts across multiple
/// `shelfbox` processes accessing the same store directory.
///
/// # Implementation note
///
/// This is a self-referential struct: `guard` borrows from `rw_lock`.
/// Heap allocation via `Box` gives `rw_lock` a stable address; `ManuallyDrop`
/// lets us control the drop order (guard first, then lock) in the custom
/// [`Drop`] implementation.
struct StoreLock {
    /// Dropped first — releases the OS advisory lock.
    guard: StoreLockGuard,
    /// Dropped second — closes the underlying file descriptor.
    rw_lock: ManuallyDrop<Box<FdRwLock<std::fs::File>>>,
}

impl std::fmt::Debug for StoreLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreLock").finish_non_exhaustive()
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        // SAFETY: Drop order is critical.
        // 1. `guard` must be dropped first to release the OS advisory lock.
        // 2. `rw_lock` is then dropped, closing the file descriptor.
        // The `Box` was never moved or freed while the guard was alive.
        unsafe {
            match &mut self.guard {
                StoreLockGuard::Write(g) => ManuallyDrop::drop(g),
                StoreLockGuard::Read(g) => ManuallyDrop::drop(g),
            }
            ManuallyDrop::drop(&mut self.rw_lock);
        }
    }
}

fn acquire_store_lock(lock_path: &Path, access: StoreAccess, create: bool) -> Result<StoreLock> {
    let file = std::fs::OpenOptions::new()
        .create(create)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|e| AppError::io(lock_path, e))?;

    let mut rw_lock = Box::new(FdRwLock::new(file));

    // SAFETY: The `Box` gives `rw_lock` a stable heap address. The raw
    // pointer is not moved or freed while the guard is alive (enforced by
    // the custom `Drop` impl above which drops the guard before the Box).
    let lock_ref: &'static mut FdRwLock<std::fs::File> =
        unsafe { &mut *(rw_lock.as_mut() as *mut _) };

    let guard = match access {
        StoreAccess::Write => {
            let g = lock_ref.write().map_err(|e| AppError::StoreLocked {
                lock_path: lock_path.to_path_buf(),
                source: Box::new(e),
            })?;
            StoreLockGuard::Write(ManuallyDrop::new(g))
        }
        StoreAccess::ReadOnly => {
            let g = lock_ref.read().map_err(|e| AppError::StoreLocked {
                lock_path: lock_path.to_path_buf(),
                source: Box::new(e),
            })?;
            StoreLockGuard::Read(ManuallyDrop::new(g))
        }
    };

    Ok(StoreLock {
        guard,
        rw_lock: ManuallyDrop::new(rw_lock),
    })
}

// ── RepoContext ───────────────────────────────────────────────────────────────

/// All context required to perform any shelfbox operation.
///
/// Built once per CLI invocation (or API call) and passed by reference to
/// every operation in `ops/`.  Keeping this struct immutable after
/// construction makes the library easier to reason about.
#[derive(Debug)]
pub struct RepoContext {
    /// Absolute path to the repository root (output of `git rev-parse
    /// --show-toplevel`).
    pub repo_root: PathBuf,

    /// ULID that uniquely identifies this repository in the global store.
    pub repo_id: String,

    /// Absolute path to the per-repo directory inside the store
    /// (`<config.store>/repos/<repo_id>/`).
    pub repo_store: PathBuf,

    /// Absolute path to the git-common-dir (output of `git rev-parse
    /// --git-common-dir`).  Stable across linked worktrees; used for
    /// cross-manifest stale detection and reclaim heuristics.
    pub git_common_dir: PathBuf,

    /// Parsed manifest for this repository.
    pub manifest: Manifest,

    /// Resolved configuration (store path, etc.).
    pub config: Config,

    /// Advisory file lock on `<store>/.lock`, held for this context's lifetime.
    /// `None` in unit-test contexts and read-only contexts that must not
    /// create lock files.
    _store_lock: Option<StoreLock>,
}

/// Store access mode for context construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreAccess {
    /// Inspect existing store state without creating store files.
    ReadOnly,
    /// Acquire write access and allow store initialization.
    Write,
}

/// Store-level context independent of any repository identity decision.
#[derive(Debug)]
pub struct StoreContext {
    pub config: Config,
    _store_lock: Option<StoreLock>,
}

/// Facts about the current Git checkout without any shelfbox store side effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentGitContext {
    pub repo_root: PathBuf,
    pub git_dir: PathBuf,
    pub git_common_dir: PathBuf,
    pub remote_hint: Option<String>,
}

/// Read-only repository lookup result.
///
/// This separates Git/config discovery from identity-changing repository
/// creation. A missing association is represented by `repo: None` instead of
/// allocating a new `RepoId`.
#[derive(Debug)]
pub struct ReadOnlyRepoContext {
    pub current: CurrentGitContext,
    pub config: Config,
    pub repo: Option<RepoContext>,
}

/// Explicit repository reclaim target.
///
/// Reclaim is the only path that may associate the current checkout with an
/// existing `RepoId` without creating a fresh identity.
#[derive(Debug, Clone)]
pub struct ExplicitReclaimContext {
    pub current: CurrentGitContext,
    pub config: Config,
    pub target_repo_id: String,
}

impl RepoContext {
    /// Root of the `items/` subdirectory inside [`repo_store`].
    pub fn items_dir(&self) -> PathBuf {
        self.repo_store.join("items")
    }

    /// Returns the absolute store path for a repo-relative `item_path`.
    ///
    /// Example: `"notes/design.md"` → `<repo_store>/items/notes/design.md`.
    pub fn store_path_for(&self, item_path: &str) -> PathBuf {
        self.items_dir().join(item_path)
    }

    /// Converts a `store_path` (absolute) back to a repo-relative string.
    pub fn store_relative_path(&self, store_path: &Path) -> Option<String> {
        store_path
            .strip_prefix(self.items_dir())
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    }
}

// ── Construction ──────────────────────────────────────────────────────────────

/// Detects Git metadata for `cwd` without creating a shelfbox `RepoId`, store
/// metadata, index entry, or manifest.
pub fn current_git_context(cwd: &Path) -> Result<CurrentGitContext> {
    let repo_root = crate::git::find_repo_root(cwd)?;
    let git_dir = crate::git::git_dir(&repo_root)?;
    let git_common_dir = crate::git::git_common_dir(&repo_root)?;
    let remote_hint =
        crate::git::remote_url(&repo_root)?.and_then(|url| crate::git::normalize_remote_hint(&url));

    Ok(CurrentGitContext {
        repo_root,
        git_dir,
        git_common_dir,
        remote_hint,
    })
}

/// Resolves an existing shelfbox repository from local Git metadata without
/// creating anything. Root matches take precedence over `git_common_dir`.
pub fn resolve_existing_repo(current: &CurrentGitContext, index: &Index) -> Option<String> {
    index
        .find_by_root(&current.repo_root)
        .or_else(|| index.find_by_git_common_dir(&current.git_common_dir))
        .map(str::to_string)
}

/// Builds a [`RepoContext`] for the current working directory.
///
/// This is the primary entry point used by CLI subcommands.  It:
/// 1. Loads configuration.
/// 2. Locates the Git repository root from `cwd`.
/// 3. Looks up (or creates) a ULID repo ID in the global index.
/// 4. Loads (or initialises) the per-repo manifest.
///
/// The `store_override` parameter lets the `--store` CLI flag take
/// precedence over values in `config.toml`.
///
/// Set `write` to `true` for commands that modify the store (`add`, `restore`,
/// `doctor --fix`, `repair`). Read-only commands (`list`, `status`, `doctor`)
/// should pass `false`.
pub fn build(cwd: &Path, store_override: Option<&Path>, write: bool) -> Result<RepoContext> {
    let access = if write {
        StoreAccess::Write
    } else {
        StoreAccess::ReadOnly
    };
    build_create_or_load_with_access(cwd, store_override, access)
}

/// Builds a [`RepoContext`] for commands that may create or load a repo identity.
pub fn build_create_or_load(cwd: &Path, store_override: Option<&Path>) -> Result<RepoContext> {
    build_create_or_load_with_access(cwd, store_override, StoreAccess::Write)
}

/// Loads store configuration and, when safe, an advisory store lock without
/// deciding which repository identity should be used.
pub fn build_store_context(
    store_override: Option<&Path>,
    access: StoreAccess,
) -> Result<StoreContext> {
    let config = Config::load(store_override)?;
    let store_lock = match access {
        StoreAccess::ReadOnly => acquire_existing_read_lock(&config.store)?,
        StoreAccess::Write => {
            meta::ensure_store_meta(&config.store)?;
            let lock_path = config.store.join(".lock");
            Some(acquire_store_lock(&lock_path, StoreAccess::Write, true)?)
        }
    };

    Ok(StoreContext {
        config,
        _store_lock: store_lock,
    })
}

fn build_create_or_load_with_access(
    cwd: &Path,
    store_override: Option<&Path>,
    access: StoreAccess,
) -> Result<RepoContext> {
    let store_context = build_initialized_store_context(store_override, access)?;
    let StoreContext {
        config,
        _store_lock: store_lock,
    } = store_context;

    // Phase 3 will fill in git::find_repo_root(); we call it via crate::git.
    let repo_root = crate::git::find_repo_root(cwd)?;

    let (repo_id, repo_store, git_common_dir, manifest) = resolve_repo(&repo_root, &config, cwd)?;

    Ok(RepoContext {
        repo_root,
        repo_id,
        repo_store,
        git_common_dir,
        manifest,
        config,
        _store_lock: store_lock,
    })
}

/// Resolves the current Git checkout against the existing store without
/// creating store metadata, a lock file, an index entry, a `RepoId`, or a
/// manifest.
pub fn build_read_only(cwd: &Path, store_override: Option<&Path>) -> Result<ReadOnlyRepoContext> {
    let store_context = build_store_context(store_override, StoreAccess::ReadOnly)?;
    let current = current_git_context(cwd)?;
    let repo = resolve_repo_read_only(&store_context.config, &current)?;

    Ok(ReadOnlyRepoContext {
        current,
        config: store_context.config,
        repo,
    })
}

pub fn build_explicit_reclaim(
    cwd: &Path,
    store_override: Option<&Path>,
    target_repo_id: impl Into<String>,
) -> Result<ExplicitReclaimContext> {
    let config = Config::load(store_override)?;
    let current = current_git_context(cwd)?;

    Ok(ExplicitReclaimContext {
        current,
        config,
        target_repo_id: target_repo_id.into(),
    })
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn build_initialized_store_context(
    store_override: Option<&Path>,
    access: StoreAccess,
) -> Result<StoreContext> {
    let config = Config::load(store_override)?;

    // Ensure the store has a meta.json identity file before acquiring the lock,
    // preserving the compatibility behavior of `build(..., false)`.
    meta::ensure_store_meta(&config.store)?;

    let lock_path = config.store.join(".lock");
    let store_lock = acquire_store_lock(&lock_path, access, true)?;

    Ok(StoreContext {
        config,
        _store_lock: Some(store_lock),
    })
}

fn acquire_existing_read_lock(store_root: &Path) -> Result<Option<StoreLock>> {
    let lock_path = store_root.join(".lock");
    if !lock_path.is_file() {
        return Ok(None);
    }

    match acquire_store_lock(&lock_path, StoreAccess::ReadOnly, false) {
        Ok(lock) => Ok(Some(lock)),
        Err(AppError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(None)
        }
        Err(err) => Err(err),
    }
}

fn resolve_repo_read_only(
    config: &Config,
    current: &CurrentGitContext,
) -> Result<Option<RepoContext>> {
    if !index::index_path(&config.store).is_file() {
        return Ok(None);
    }

    let index = index::load(&config.store)?;
    let Some(repo_id) = resolve_existing_repo(current, &index) else {
        return Ok(None);
    };

    let Some(entry) = index.get(&repo_id) else {
        return Ok(None);
    };

    let repo_store = config.store.join("repos").join(&entry.repo_store_dir);
    if !manifest::manifest_path(&repo_store).is_file() {
        return Ok(None);
    }

    let manifest = manifest::load(&repo_store)?;
    Ok(Some(RepoContext {
        repo_root: current.repo_root.clone(),
        repo_id,
        repo_store,
        git_common_dir: current.git_common_dir.clone(),
        manifest,
        config: config.clone(),
        _store_lock: None,
    }))
}

/// Resolves (or creates) the repo ID and loads (or initialises) the manifest.
///
/// Returns `(repo_id, repo_store, git_common_dir, manifest)`.
fn resolve_repo(
    repo_root: &Path,
    config: &Config,
    cwd: &Path,
) -> Result<(String, PathBuf, PathBuf, Manifest)> {
    let store_root = &config.store;
    let mut index = index::load(store_root)?;

    // Determine the git-common-dir once; needed for both identity lookup and
    // for the new RepoEntry when this repository is seen for the first time.
    // Fall back to `repo_root/.git` if the git command fails (e.g. in tests
    // that create a bare-minimum repo without worktrees).
    let git_dir = crate::git::git_dir(repo_root).unwrap_or_else(|_| repo_root.join(".git"));
    let common_dir = crate::git::git_common_dir(repo_root).unwrap_or_else(|_| git_dir.clone());

    // Human-readable portion of the store directory name (used for new repos).
    let repo_name = repo_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".into());

    // Two-stage lookup:
    // 1. Exact root match (fast path, normal case).
    // 2. git-common-dir match (handles linked worktrees and moved repos).
    let (repo_id, store_dir) = if let Some(id) = index.find_by_root(repo_root) {
        let id = id.to_string();
        let dir = index
            .get(&id)
            .map(|e| e.repo_store_dir.clone())
            .unwrap_or_else(|| id.clone());
        (id, dir)
    } else if let Some(id) = index.find_by_git_common_dir(&common_dir) {
        // Repo was accessed via a different worktree or root path changed;
        // update the recorded root to the current one.
        let id = id.to_string();
        let dir = if let Some(entry) = index.get(&id) {
            let dir = entry.repo_store_dir.clone();
            let mut updated = entry.clone();
            updated.root = Some(repo_root.to_path_buf());
            updated.git_dir = Some(git_dir.clone());
            updated.git_common_dir = Some(common_dir.clone());
            updated.last_seen_at = now_iso8601();
            index.upsert(&id, updated);
            index::save(store_root, &index)?;
            dir
        } else {
            id.clone()
        };
        (id, dir)
    } else {
        // First time this repository is seen: generate a new ULID and a
        // human-readable directory name.
        let id = Ulid::new().to_string();
        let dir = allocate_repo_store_dir(store_root, &sanitize_name(&repo_name), &id)?;
        let entry = RepoEntry {
            root: Some(repo_root.to_path_buf()),
            git_dir: Some(git_dir.clone()),
            git_common_dir: Some(common_dir.clone()),
            repo_store_dir: dir.clone(),
            last_seen_at: now_iso8601(),
        };
        index.upsert(&id, entry);
        index::save(store_root, &index)?;
        (id, dir)
    };

    let repo_store = store_root.join("repos").join(&store_dir);

    // Update `last_seen_at` on every access so doctor/status can detect
    // repositories that have vanished.
    update_last_seen(store_root, &mut index, &repo_id)?;

    let manifest = load_or_init_manifest(&repo_store, &repo_id, &repo_name, cwd)?;

    Ok((repo_id, repo_store, common_dir, manifest))
}

fn update_last_seen(store_root: &Path, index: &mut Index, repo_id: &str) -> Result<()> {
    if let Some(entry) = index.get(repo_id) {
        let mut updated = entry.clone();
        updated.last_seen_at = now_iso8601();
        index.upsert(repo_id, updated);
        index::save(store_root, index)?;
    }
    Ok(())
}

fn load_or_init_manifest(
    repo_store: &Path,
    repo_id: &str,
    repo_name: &str,
    _cwd: &Path,
) -> Result<Manifest> {
    let path = manifest::manifest_path(repo_store);
    if path.exists() {
        manifest::load(repo_store)
    } else {
        let mut manifest = Manifest::new(repo_id.to_string(), now_iso8601());
        manifest.add_repo_name_hint(repo_name);
        Ok(manifest)
    }
}

fn allocate_repo_store_dir(store_root: &Path, base_name: &str, repo_id: &str) -> Result<String> {
    let repos_dir = store_root.join("repos");
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

fn repo_store_dir_available(repos_dir: &Path, candidate: &str, repo_id: &str) -> Result<bool> {
    let repo_store = repos_dir.join(candidate);
    if !repo_store.exists() {
        return Ok(true);
    }

    let manifest_path = manifest::manifest_path(&repo_store);
    let contents = match std::fs::read_to_string(&manifest_path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(AppError::io(manifest_path, e)),
    };
    let raw: serde_json::Value =
        serde_json::from_str(&contents).map_err(|e| AppError::json(manifest_path, e))?;

    Ok(raw.get("repo_id").and_then(|v| v.as_str()) == Some(repo_id))
}

// ── Utilities ─────────────────────────────────────────────────────────────────

/// Converts a repository name to a filesystem-safe ASCII slug.
///
/// Non-alphanumeric characters are replaced with `-`; consecutive dashes are
/// collapsed; leading and trailing dashes are stripped.  If the result would
/// be empty, `"repo"` is returned as a safe fallback.
fn sanitize_name(name: &str) -> String {
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

/// Returns the current UTC time as a naïve ISO-8601 string.
///
/// Uses only `std` to avoid pulling in `chrono` or `time` for a single use.
pub fn now_iso8601() -> String {
    // std::time gives us seconds since UNIX epoch; convert manually.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    epoch_secs_to_iso8601(secs)
}

fn epoch_secs_to_iso8601(secs: u64) -> String {
    // Naïve UTC conversion (no leap-second handling needed for timestamps).
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86400;

    // Gregorian calendar calculation from day count since 1970-01-01.
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm: civil date from UNIX day count.
    let z = days + 719468;
    let era = z / 146097;
    let doe = z % 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    use tempfile::TempDir;

    fn init_git_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        init_git_repo_at(dir.path());
        dir
    }

    fn init_git_repo_at(path: &Path) {
        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "user.name", "Test User"],
        ] {
            run_git(path, &args);
        }
    }

    fn init_git_repo_with_commit() -> TempDir {
        let dir = init_git_repo();
        run_git(dir.path(), &["commit", "--allow-empty", "-m", "initial"]);
        dir
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let output = StdCommand::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap_or_else(|e| panic!("failed to spawn git {}: {e}", args[0]));
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args[0],
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn epoch_zero_is_unix_epoch() {
        assert_eq!(epoch_secs_to_iso8601(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn known_timestamp() {
        // 2026-04-29 00:00:00 UTC = 1777420800
        assert_eq!(epoch_secs_to_iso8601(1_777_420_800), "2026-04-29T00:00:00Z");
    }

    #[test]
    fn store_path_for_roundtrips() {
        use crate::{config::Config, store::manifest::Manifest};

        let repo_store = PathBuf::from("/store/repos/TESTID");
        let ctx = RepoContext {
            repo_root: PathBuf::from("/repo"),
            repo_id: "TESTID".into(),
            repo_store: repo_store.clone(),
            git_common_dir: PathBuf::from("/repo/.git"),
            manifest: Manifest::new("TESTID", "2026-04-29T00:00:00Z"),
            config: Config::with_store("/store"),
            _store_lock: None,
        };

        let abs = ctx.store_path_for("notes/design.md");
        assert_eq!(
            abs,
            PathBuf::from("/store/repos/TESTID/items/notes/design.md")
        );

        let rel = ctx.store_relative_path(&abs).unwrap();
        assert_eq!(rel, "notes/design.md");
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
        manifest::save(&existing, &manifest).unwrap();

        let dir = allocate_repo_store_dir(store.path(), "my-project", "repo-2").unwrap();

        assert_eq!(dir, "my-project-2");
    }

    #[test]
    fn repo_store_dir_three_way_conflict_uses_next_numeric_suffix() {
        let store = tempfile::TempDir::new().unwrap();
        for (dir, repo_id) in [("my-project", "repo-1"), ("my-project-2", "repo-2")] {
            let repo_store = store.path().join("repos").join(dir);
            let manifest = Manifest::new(repo_id, "2026-04-29T00:00:00Z");
            manifest::save(&repo_store, &manifest).unwrap();
        }

        let dir = allocate_repo_store_dir(store.path(), "my-project", "repo-3").unwrap();

        assert_eq!(dir, "my-project-3");
    }

    #[test]
    fn repo_store_dir_allows_existing_dir_with_same_repo_id() {
        let store = tempfile::TempDir::new().unwrap();
        let existing = store.path().join("repos/my-project");
        let manifest = Manifest::new("repo-1", "2026-04-29T00:00:00Z");
        manifest::save(&existing, &manifest).unwrap();

        let dir = allocate_repo_store_dir(store.path(), "my-project", "repo-1").unwrap();

        assert_eq!(dir, "my-project");
    }

    #[test]
    fn current_git_context_resolves_indexed_repo_by_root() {
        let repo = init_git_repo();
        run_git(
            repo.path(),
            &["remote", "add", "origin", "git@github.com:example/app.git"],
        );
        let current = current_git_context(repo.path()).unwrap();

        let mut index = Index::new();
        index.upsert(
            "repo-1",
            RepoEntry {
                root: Some(current.repo_root.clone()),
                git_dir: Some(current.git_dir.clone()),
                git_common_dir: Some(current.git_common_dir.clone()),
                repo_store_dir: "app".into(),
                last_seen_at: "2026-04-29T00:00:00Z".into(),
            },
        );

        assert_eq!(
            current.remote_hint.as_deref(),
            Some("github.com/example/app")
        );
        assert_eq!(
            resolve_existing_repo(&current, &index).as_deref(),
            Some("repo-1")
        );
    }

    #[test]
    fn current_git_context_for_unindexed_clone_creates_no_store_files() {
        let repo = init_git_repo();
        let store = TempDir::new().unwrap();

        let current = current_git_context(repo.path()).unwrap();
        let index = Index::new();

        assert_eq!(resolve_existing_repo(&current, &index), None);
        assert!(!store.path().join("index.json").exists());
        assert!(!store.path().join("repos").exists());
        assert!(!store.path().join("meta.json").exists());
    }

    #[test]
    fn current_git_context_resolves_linked_worktree_by_git_common_dir() {
        let main = init_git_repo_with_commit();
        let worktree_parent = TempDir::new().unwrap();
        let worktree = worktree_parent.path().join("linked-worktree");
        run_git(
            main.path(),
            &["worktree", "add", worktree.to_str().unwrap(), "HEAD"],
        );

        let main_current = current_git_context(main.path()).unwrap();
        let worktree_current = current_git_context(&worktree).unwrap();

        let mut index = Index::new();
        index.upsert(
            "repo-1",
            RepoEntry {
                root: Some(main_current.repo_root),
                git_dir: Some(main_current.git_dir),
                git_common_dir: Some(main_current.git_common_dir),
                repo_store_dir: "app".into(),
                last_seen_at: "2026-04-29T00:00:00Z".into(),
            },
        );

        assert_eq!(
            resolve_existing_repo(&worktree_current, &index).as_deref(),
            Some("repo-1")
        );
    }

    #[test]
    fn current_git_context_outside_git_repo_returns_not_a_git_repo() {
        let dir = TempDir::new().unwrap();

        let err = current_git_context(dir.path()).unwrap_err();

        assert!(matches!(err, AppError::NotAGitRepo));
    }
}
