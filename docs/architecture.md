# Architecture

## High-level overview

`shelfbox` is a Cargo workspace:

```
shelfbox/
├── Cargo.toml                  # workspace root
└── crates/
    ├── shelfbox-core/       # library crate — all business logic
    └── shelfbox/            # binary crate — CLI only
```

The separation enforces a clean boundary: the library has no knowledge of
argument parsing or human-readable output, and the binary has no knowledge of
disk structures or OS calls.

---

## Crate layout

### `shelfbox-core` (library)

```
src/
  lib.rs          # re-exports public API
  error.rs        # AppError enum (thiserror)
  config.rs       # Config load / XDG paths / set_key() for atomic TOML writes
  context.rs      # RepoContext + context::build()
  git.rs          # Git plumbing (std::process::Command only)
  ignore.rs       # IgnoreBackend trait + GitInfoExclude impl
  link.rs         # LinkStrategy trait + SymlinkStrategy impl
  store/
    mod.rs
    index.rs      # Index (ULID → RepoEntry), atomic JSON save
    manifest.rs   # Manifest, Item, ItemKind, LinkInfo, GitInfo
  ops/
    mod.rs
    add.rs        # add() — shelve a path
    restore.rs    # restore() — unshelve a path
    list.rs       # list() — read manifest items
    status.rs     # status() — per-item health check
    integrity.rs  # check() / fix() — integrity report and repair
    repair.rs     # repair() — single-file symlink repair
    info.rs       # info() — single-item diagnostic metadata
```

### `shelfbox` (binary)

```
src/
  main.rs           # entry point — fn main() -> ExitCode, maps Ok(code)/Err to exit
  cli.rs            # Cli struct, Command enum (5 groups + hidden Doctor alias), run() dispatcher
  cmd/
    mod.rs          # re-exports all cmd modules
    item.rs         # item subcommand logic (add, restore, repair, list, status, info)
    repo.rs         # repo subcommand logic (list, status, repair, gc)
    store.rs        # store subcommand logic (info, verify, gc)
    config.rs       # config subcommand logic (get, set, path)
    internal.rs     # internal subcommand logic (debug with path masking, completions)
    format.rs       # OutputFormat enum (Table, Plain, Json, Detail)
    util.rs         # path resolution helpers
```

The binary crate depends on `clap` (CLI parsing), `clap_complete` (shell
completions), `anyhow` (error formatting), `serde_json` (JSON output), and
`shelfbox-core` (all business logic).

---

## Data model

### Global index — `index.json`

Stored at `<store>/index.json`.  One entry per repository ever seen.

```json
{
  "version": 1,
  "repos": {
    "01JTARXXXXXXXXXXXXXXXX": {
      "root": "/home/user/projects/myapp",
      "git_dir": "/home/user/projects/myapp/.git",
      "git_common_dir": "/home/user/projects/myapp/.git",
      "store_dir": "myapp-01JTARXXXXXXXXXXXXXXXX",
      "last_seen_at": "2026-04-29T12:00:00Z"
    }
  }
}
```

The ULID key is stable for the lifetime of the repository entry.  `root` is an
absolute path; a mismatch between `root` and the current working Git root is
reported by `repo status` as a warning.  `git_common_dir` is the output of
`git rev-parse --git-common-dir`, which is the same as `git_dir` for a normal
clone and points to the main clone's `.git/` for a linked worktree.

### Per-repository manifest — `manifest.json`

Stored at `<store>/repos/<sanitized-name>-<ULID>/manifest.json`.

```json
{
  "version": 1,
  "repo": {
    "id": "01JTARXXXXXXXXXXXXXXXX",
    "name": "myapp",
    "remote": "https://github.com/user/myapp"
  },
  "items": [
    {
      "path": ".env",
      "store_path": "items/.env",
      "kind": "File",
      "link": { "link_type": "Symlink" },
      "git": { "was_tracked": false },
      "created_at": "2026-04-29T12:00:00Z",
      "updated_at": "2026-04-29T12:00:00Z"
    }
  ]
}
```

`store_path` is a **path relative to `repo_store`** (e.g. `"items/.env"`) and is
joined with the absolute `repo_store` at runtime to locate the store-side item.
Using a relative path keeps `manifest.json` portable: it can be copied to
another machine or a different store root without modification.

### Store directory tree

```
~/.local/share/shelfbox/
  meta.json            # store identity: store_id ULID + created_at
  index.json
  repos/
    myapp-01JTAR…/     # <sanitized-name>-<ULID>
      manifest.json
      items/           # mirrors the repository's relative path structure
        .env
        secrets/
          api_key.txt
```

`items/` preserves the exact relative path of each shelved item, making the
contents human-readable and easy to inspect manually.

---

## Request lifecycle

### `shelfbox item add notes.md`

```
CLI parse (clap)
  └─ cli::run() → cmd::item::run_item()
       ├─ context::build(cwd, store_override, write=true)
       |    ├─ git::find_repo_root(cwd)               // git rev-parse --show-toplevel
       |    ├─ Config::load(store_override)           // XDG config file
       |    ├─ Index::load(store_root)                // deserialize index.json
       |    └─ Index::upsert(repo_root)               // emit or reuse ULID
       └─ ops::add::add(ctx, path, dry_run, link, ignore)
            ├─ validate — 7 checks (see user guide)
            ├─ move file → store
            ├─ link.create(original, store_dest)      // SymlinkStrategy::create
            ├─ manifest.items.push(item)
            ├─ manifest::save(store, manifest)
            ├─ ignore.add_entries([rel_path])         // GitInfoExclude
            └─ index::save(store_root, index)
```

### `shelfbox repo status`

```
cmd::repo::run_repo()
  ├─ context::build(cwd, store_override, write=false)
  ├─ ops::integrity::check(ctx, link, ignore)
  |    ├─ ops::status::status()                       // per-item checks
  |    ├─ collect_orphan_store_items()                // walk items/ dir
  |    └─ check_repo_root_in_index()                  // load index, compare root
  ├─ print_repo_status(report, repo_root)           // or detail/json/plain variant
  └─ classify_integrity_exit(&report)               // returns ExitCode 0 / 1 / 2
```

### `shelfbox repo repair`

```
cmd::repo::run_repo()
  ├─ context::build(cwd, store_override, write=true)
  ├─ ops::integrity::fix(ctx, link, ignore, yes=false, dry_run)
  |    ├─ fix_root_mismatch()                        // update index root
  |    ├─ rebuild_manifest_from_store()              // absorb orphans into manifest
  |    ├─ fix_exclude_entries()                      // re-add missing exclude entries
  |    ├─ fix_symlinks()                             // repair() for each broken link
  |    └─ handle_orphans()                           // confirm remaining orphans
  └─ print_fix_result(result) for each FixResult
```

### `shelfbox item repair <PATH>`

```
cmd::item::run_item()
  ├─ context::build(cwd, store_override, write=true)
  └─ ops::repair::repair(ctx, abs_path, link, dry_run)
       ├─ manifest.get(rel_path)                     // NotManaged if absent
       ├─ store_path.exists()                        // StoreMissing if absent
       ├─ link.is_managed_link()                     // AlreadyHealthy if OK
       ├─ [safety] regular file check                // Err(PathIsRegularFile) if present
       ├─ remove existing symlink (if any)
       └─ link.create(store_path, abs_path)          // LinkRecreated
```

---

## Design decisions

| Decision | Rationale |
|---|---|
| **No `git2` dependency** | `std::process::Command` is sufficient for the two queries needed (`rev-parse`, `ls-files`). Avoids a large C dependency and keeps compile times short. |
| **ULID for repo IDs** | Monotonically sortable, URL-safe, collision-resistant, and human-readable without a database. |
| **Atomic index and manifest writes** | Both `index.json` and `manifest.json` are written via a temp-file-then-rename strategy. `rename(2)` is atomic on POSIX; a crash mid-write cannot corrupt either file. |
| **`thiserror` in lib / `anyhow` in bin** | Library errors should be typed and stable for callers; binary errors only need to be printable. Mixing them would pollute the library API. |
| **`store_path` in manifest is repo-store-relative** | Using a relative path (e.g. `"items/.env"`) instead of an absolute one keeps `manifest.json` portable across machines and store relocations. The full path is reconstructed at runtime as `ctx.repo_store.join(&item.store_path)`. |
| **Exclude entries are sorted** | Deterministic ordering makes diffs human-readable and prevents spurious changes to `.git/info/exclude`. |
| **`store_path` layout is deterministic** | `items/<repo-relative-path>` makes manifest reconstruction from store trivially reversible without any guesswork. |
| **`fix` is ordered and best-effort** | Root fix runs first so path comparisons are correct; each step continues even if a previous one fails, recording `FixResult::Failed` entries rather than aborting. |
| **Navigation hints are CLI-only** | `shelfbox-core` returns structured `IntegrityReport` / `IntegrityFixReport` data; formatting, hints, and TTY detection live entirely in the binary crate. |
| **`repair` refuses to overwrite regular files** | If a non-symlink file exists at the target path, `repair` returns `Err(PathIsRegularFile)` rather than silently overwriting user data. |
| **`# BEGIN shelfbox` block in exclude** | All shelfbox entries are wrapped in a named block so other tools can safely edit the file without conflict. The block is rewritten atomically; existing content outside the block is preserved. |
| **`LinkStrategy` abstraction** | All filesystem linking is dispatched through the `LinkStrategy` trait. Today only `SymlinkStrategy` (Unix symlinks) is shipped. Future implementations (hardlink, bind mount, copy mode) can be added without touching `ops/`. |
| **Worktree-aware repo identity** | `RepoEntry` stores both `git_dir` and `git_common_dir` (output of `git rev-parse --git-common-dir`). Repo lookup uses a two-stage strategy: exact `root` match first, then `git_common_dir` match. This ensures that accessing a repository via a linked worktree reuses the same ULID rather than creating a duplicate entry. `exclude_file_path` is also resolved via `git rev-parse --git-path info/exclude` so the correct `.git/info/exclude` is targeted in worktree environments. |
| **`<name>-<ULID>` repo store directories** | Per-repo store directories are named `<sanitized-repo-name>-<ULID>` (e.g. `my-project-01JTAR…`). The human-readable prefix makes the store legible with `ls` while the ULID suffix guarantees global uniqueness. Non-alphanumeric characters in the repo name are replaced with `-`. The `store_dir` field is persisted in `index.json` so the directory name never changes after first creation. |
| **Store identity via `meta.json`** | `<store>/meta.json` is written on first use. It contains a ULID `store_id` and `created_at` timestamp. This provides a stable identity for the store, useful for diagnosing relocations and generating informative error messages. The write is idempotent: subsequent runs leave the file unchanged. |
| **`SHELFBOX_STORE` environment variable** | The store root can be set via the `SHELFBOX_STORE` environment variable, overriding `config.toml` but yielding to the `--store` CLI flag. Priority: `--store` > `$SHELFBOX_STORE` > `config.toml` > XDG default. This follows the UNIX convention of env-var config and enables easy store switching in shell sessions without editing config files. |
| **Store-level advisory file lock** | `context::build()` acquires an advisory `flock` on `<store>/.lock` before reading or writing any store data. Write commands (`add`, `restore`, `repo repair`, `item repair`) acquire an exclusive lock; read commands (`list`, `status`, `repo status`) acquire a shared lock. This prevents index/manifest inconsistency when two `shelfbox` processes run concurrently against the same store. The lock is released when `RepoContext` is dropped (end of the command). If the lock cannot be acquired an `AppError::StoreLocked` is returned with a human-readable hint. |
| **Machine-readable exit codes** | `repo status` returns 0/1/2 and `store verify` returns 0/2 based on the severity of issues found. All other commands return 0 on success. Any unhandled `anyhow::Error` produces exit code 255. `fn main()` is typed as `-> ExitCode` rather than `-> Result<()>` so the process exit value is always explicit. |
| **`internal debug` path masking** | By default, `internal debug` replaces the home directory prefix in all output paths with `~`, reducing the risk of leaking absolute paths in bug reports or AI chats. The `--allow-sensitive` flag disables masking for use in scripts that need raw paths. |
| **`config set` uses `toml_edit`** | `config set <key> <value>` patches the TOML config file using `toml_edit::DocumentMut` (parse → patch → atomic rename) rather than full re-serialisation. This preserves user comments and unknown keys. Only keys that correspond to a known `Config` field are accepted; others return an error. |
| **`doctor` as a hidden alias** | `shelfbox doctor` is registered as a hidden `Command::Doctor` variant that delegates to `run_repo(RepoCommand::Status { format })`. It is invisible in `--help` but fully functional, following the `brew doctor` / `flutter doctor` convention. |
| **No multi-machine sync support** | shelfbox deliberately does not support synchronising the store across machines. Correct distributed sync requires conflict resolution, merge semantics, operation logs, and crash recovery — effectively a mini distributed database — which is out of scope. The store layout is instead designed to be **resilient to single-machine partial failures**: atomic writes prevent mid-write corruption, and `repo repair` can fully reconstruct the manifest from the deterministic `items/` layout. Users who place the store in a synced folder (e.g. Dropbox) are advised to do so on one machine at a time; `repo repair` is the recovery path if a sync collision occurs. |
| **`repo repair` rebuild candidate requires exact symlink target** | A store item without a manifest entry is treated as a rebuild candidate only if the symlink at the expected repo-relative path points **exactly** to `<repo_store>/items/<path>`. A symlink with a different target (e.g. from a re-clone pointing to an old store, or an unrelated tool's symlink) is treated as an orphan and not absorbed. This guards against incorrect manifest reconstruction caused by stale or coincidental symlinks. |

## Repair policy

The following table defines the contract for `shelfbox repair` depending on the state found at the repo-side path.

| State at repo-side path | `repair` behaviour |
|---|---|
| Missing (no file) | Recreate the symlink pointing to the store item. |
| Dangling symlink (target deleted) | Remove the dangling symlink and recreate it. |
| Wrong-target symlink (points elsewhere) | Remove and recreate (the symlink is considered stale). **Note:** this is the current unguarded behaviour; a future version may refuse by default and require `--force`. |
| Regular file (not a symlink) | Return `Err(PathIsRegularFile)` — refuse to overwrite user data. |
| Directory | Return `Err(PathIsRegularFile)` — refuse to overwrite. |
| Already healthy (correct symlink) | No-op, return `Ok(AlreadyHealthy)`. |
