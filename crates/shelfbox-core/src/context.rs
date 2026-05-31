use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};

use fd_lock::{RwLock as FdRwLock, RwLockReadGuard, RwLockWriteGuard};
use ulid::Ulid;

use crate::{
    config::Config,
    error::{AppError, Result},
    store::{
        index::{self, Index, RepoEntry},
        manifest::{self, Manifest, RepoMeta},
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

fn acquire_store_lock(lock_path: &Path, write: bool) -> Result<StoreLock> {
    let file = std::fs::OpenOptions::new()
        .create(true)
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

    let guard = if write {
        let g = lock_ref.write().map_err(|e| AppError::StoreLocked {
            lock_path: lock_path.to_path_buf(),
            source: e,
        })?;
        StoreLockGuard::Write(ManuallyDrop::new(g))
    } else {
        let g = lock_ref.read().map_err(|e| AppError::StoreLocked {
            lock_path: lock_path.to_path_buf(),
            source: e,
        })?;
        StoreLockGuard::Read(ManuallyDrop::new(g))
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
    /// `None` only in unit-test contexts that construct `RepoContext` directly.
    _store_lock: Option<StoreLock>,
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
    let config = Config::load(store_override)?;

    // Ensure the store has a meta.json identity file before anything else.
    meta::ensure_store_meta(&config.store)?;

    // Acquire an advisory lock on the store so concurrent invocations do not
    // interleave index and manifest writes.
    let lock_path = config.store.join(".lock");
    let store_lock = acquire_store_lock(&lock_path, write)?;

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
        _store_lock: Some(store_lock),
    })
}

// ── Internal helpers ──────────────────────────────────────────────────────────

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
    let common_dir =
        crate::git::git_common_dir(repo_root).unwrap_or_else(|_| repo_root.join(".git"));

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
            .map(|e| e.store_dir.clone())
            .unwrap_or_else(|| id.clone());
        (id, dir)
    } else if let Some(id) = index.find_by_git_common_dir(&common_dir) {
        // Repo was accessed via a different worktree or root path changed;
        // update the recorded root to the current one.
        let id = id.to_string();
        let dir = if let Some(entry) = index.get(&id) {
            let dir = entry.store_dir.clone();
            let mut updated = entry.clone();
            updated.root = repo_root.to_path_buf();
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
        let dir = format!("{}-{}", sanitize_name(&repo_name), &id);
        let git_dir = common_dir.clone();
        let entry = RepoEntry {
            root: repo_root.to_path_buf(),
            git_dir,
            git_common_dir: common_dir.clone(),
            store_dir: dir.clone(),
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
        // Remote URL is filled in by Phase 3 (git::get_remote_url).
        // Leave it as None for now; it will be updated on first `add`.
        let meta = RepoMeta {
            id: repo_id.to_string(),
            name: repo_name.to_string(),
            remote: None,
        };
        Ok(Manifest::new(meta))
    }
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
            manifest: Manifest::new(RepoMeta {
                id: "TESTID".into(),
                name: "repo".into(),
                remote: None,
            }),
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
}
