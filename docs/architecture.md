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
    add.rs                # add() / add_directory() — shelve a file or directory namespace
    restore.rs            # restore() / restore_namespace() — unshelve a file or namespace
    adopt.rs              # adopt() — transfer items from another repo identity
    relink.rs             # relink() — re-attach a detached item (detached → attached)
    list.rs               # list() — read manifest items
    status.rs             # status() — per-item health check
    integrity.rs          # check() / fix() — integrity report and repair
    repair.rs             # repair() — single-file symlink repair
    move_item.rs          # move_item() — rename a tracked path
    info.rs               # info() — single-item diagnostic metadata
    detect_transitions.rs # run() / scan() — automatic Attached→Stale/Unreachable detection
```

### `shelfbox` (binary)

```
src/
  main.rs           # entry point — fn main() -> ExitCode, maps Ok(code)/Err to exit
  cli.rs            # Cli struct, Command enum (5 groups + hidden Doctor alias), run() dispatcher
  cmd/
    mod.rs          # re-exports all cmd modules
    item.rs         # item subcommand logic (add, restore, repair, list, status, move, info)
    repo.rs         # repo subcommand logic (list, status, repair, gc, adopt)
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
  "version": 2,
  "repo": {
    "id": "01JTARXXXXXXXXXXXXXXXX",
    "name": "myapp",
    "remote": "https://github.com/user/myapp"
  },
  "items": [
    {
      "item_id": "01JTBRXXXXXXXXXXXXXXXX",
      "origin_repo_id": "01JTARXXXXXXXXXXXXXXXX",
      "path": ".env",
      "store_path": "items/.env",
      "kind": "file",
      "link": { "type": "symlink" },
      "git": { "was_tracked": false },
      "ownership_state": "attached",
      "created_at": "2026-04-29T12:00:00Z",
      "updated_at": "2026-04-29T12:00:00Z"
    }
  ],
  "namespaces": [
    {
      "path": "secrets/",
      "created_at": "2026-04-29T12:02:00Z",
      "updated_at": "2026-04-29T12:02:00Z"
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
  meta.json            # store identity: store_id ULID + created_at + hostname (provenance)
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
  ├─ ops::detect_transitions::run(ctx, config)       // Attached→Stale/Unreachable in other repos
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
  └─ ops::repair::repair(ctx, abs_path, link, dry_run, force)
       ├─ manifest.get(rel_path)                     // NotManaged if absent
       ├─ store_path.exists()                        // StoreMissing if absent
       ├─ link.is_managed_link()                     // AlreadyHealthy if OK
       ├─ [safety] regular file check                // Err(PathIsRegularFile) if present
       ├─ [safety] wrong-target symlink check        // Err(RepairSymlinkTargetMismatch) unless force=true
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
| **`repair` requires `--force` for wrong-target symlinks** | If a symlink exists at the repo path but points to an unexpected target (outside the managed store), `repair` returns `Err(RepairSymlinkTargetMismatch)` unless `force = true`. This is consistent with `move_item`'s `MoveSourceSymlinkMismatch` guard and with `rebuild_manifest_from_store`'s exact-target requirement. Silent overwrite would mask stale links from reclones, store relocations, or copied repos — situations the user should investigate before proceeding. `integrity::fix` always passes `force = false`, keeping automated repair conservative. |
| **`# BEGIN shelfbox` block in exclude** | All shelfbox entries are wrapped in a named block so other tools can safely edit the file without conflict. The block is rewritten atomically; existing content outside the block is preserved. |
| **`LinkStrategy` abstraction** | All filesystem linking is dispatched through the `LinkStrategy` trait. Today only `SymlinkStrategy` (Unix symlinks) is shipped. Future implementations (hardlink, bind mount, copy mode) can be added without touching `ops/`. |
| **Worktree-aware repo identity** | `RepoEntry` stores both `git_dir` and `git_common_dir` (output of `git rev-parse --git-common-dir`). Repo lookup uses a two-stage strategy: exact `root` match first, then `git_common_dir` match. This ensures that accessing a repository via a linked worktree reuses the same ULID rather than creating a duplicate entry. `exclude_file_path` is also resolved via `git rev-parse --git-path info/exclude` so the correct `.git/info/exclude` is targeted in worktree environments. |
| **`<name>-<ULID>` repo store directories** | Per-repo store directories are named `<sanitized-repo-name>-<ULID>` (e.g. `my-project-01JTAR…`). The human-readable prefix makes the store legible with `ls` while the ULID suffix guarantees global uniqueness. Non-alphanumeric characters in the repo name are replaced with `-`. The `store_dir` field is persisted in `index.json` so the directory name never changes after first creation. |
| **Store identity via `meta.json`** | `<store>/meta.json` is written on first use. It contains a ULID `store_id`, `created_at` timestamp, and `hostname` (the creating machine's hostname recorded for provenance display only — never used as an identity source). The write is idempotent: subsequent runs leave the file unchanged. Old stores without `hostname` deserialise cleanly via `#[serde(default)]`. |
| **`SHELFBOX_STORE` environment variable** | The store root can be set via the `SHELFBOX_STORE` environment variable, overriding `config.toml` but yielding to the `--store` CLI flag. Priority: `--store` > `$SHELFBOX_STORE` > `config.toml` > XDG default. This follows the UNIX convention of env-var config and enables easy store switching in shell sessions without editing config files. |
| **Store-level advisory file lock** | `context::build()` acquires an advisory `flock` on `<store>/.lock` before reading or writing any store data. Write commands (`add`, `restore`, `repo repair`, `item repair`) acquire an exclusive lock; read commands (`list`, `status`, `repo status`) acquire a shared lock. This prevents index/manifest inconsistency when two `shelfbox` processes run concurrently against the same store. The lock is released when `RepoContext` is dropped (end of the command). If the lock cannot be acquired an `AppError::StoreLocked` is returned with a human-readable hint. |
| **Machine-readable exit codes** | `repo status` returns 0/1/2 and `store verify` returns 0/2 based on the severity of issues found. All other commands return 0 on success. Any unhandled `anyhow::Error` produces exit code 255. `fn main()` is typed as `-> ExitCode` rather than `-> Result<()>` so the process exit value is always explicit. |
| **`internal debug` path masking** | By default, `internal debug` replaces the home directory prefix in all output paths with `~`, reducing the risk of leaking absolute paths in bug reports or AI chats. The `--allow-sensitive` flag disables masking for use in scripts that need raw paths. |
| **`config set` uses `toml_edit`** | `config set <key> <value>` patches the TOML config file using `toml_edit::DocumentMut` (parse → patch → atomic rename) rather than full re-serialisation. This preserves user comments and unknown keys. Only keys that correspond to a known `Config` field are accepted; others return an error. |
| **`doctor` as a hidden alias** | `shelfbox doctor` is registered as a hidden `Command::Doctor` variant that delegates to `run_repo(RepoCommand::Status { format })`. It is invisible in `--help` but fully functional, following the `brew doctor` / `flutter doctor` convention. |
| **No multi-machine sync support** | shelfbox deliberately does not support synchronising the store across machines. Correct distributed sync requires conflict resolution, merge semantics, operation logs, and crash recovery — effectively a mini distributed database — which is out of scope. The store layout is instead designed to be **resilient to single-machine partial failures**: atomic writes prevent mid-write corruption, and `repo repair` can fully reconstruct the manifest from the deterministic `items/` layout. Users who place the store in a synced folder (e.g. Dropbox) are advised to do so on one machine at a time; `repo repair` is the recovery path if a sync collision occurs. |
| **`repo repair` rebuild candidate requires exact symlink target** | A store item without a manifest entry is treated as a rebuild candidate only if the symlink at the expected repo-relative path points **exactly** to `<repo_store>/items/<path>`. A symlink with a different target (e.g. from a re-clone pointing to an old store, or an unrelated tool's symlink) is treated as an orphan and not absorbed. This guards against incorrect manifest reconstruction caused by stale or coincidental symlinks. |
| **`item_id` is ownership identity, not content identity** | Each item receives a fresh ULID at shelve time. The same file shelved twice (after an intermediate restore) gets a different `item_id`. This keeps the identity stable across renames and repository moves (`origin_repo_id` is immutable), without requiring content hashing. |
| **`origin_repo_id` is immutable** | It records the repo that first shelved the item and is never changed — not even by `repo adopt`. After adoption, the new manifest entry carries the original `origin_repo_id`, making item provenance always traceable. |
| **Manifest version 2: ownership fields** | `item_id`, `origin_repo_id`, and `ownership_state` were added in version 2. Reading a v1 manifest is rejected at load time with `UnsupportedManifestVersion`. |
| **`namespaces` uses `#[serde(default)]`** | The `namespaces` array was added in v2 with `#[serde(default)]`. Existing v2 manifests without the key deserialize to `namespaces: []` without error — no version bump was needed. |
| **Namespace membership is derived, not stored** | A manifest item belongs to a namespace if `item.path.starts_with(&namespace.path)`. There is no stored member list. Membership queries are O(items × namespaces) but both are expected to be small. |
| **Namespace entries are not recovered by `repo repair`** | `rebuild_manifest_from_store` reconstructs `items` from store files but sets `namespaces: []`. Users re-register namespaces by re-running `item add <dir>/` (the files are already managed; only the grouping entry is recreated). Storing membership would require a separate truth source to reconstruct, adding complexity for limited benefit. |
| **`repo adopt` copies, does not move, store files** | Adoption copies the store file into the current repo's store directory and updates both manifests atomically. The source manifest marks the item `adopted`. The physical copy means the source item remains intact for auditability and in case of adoption rollback. |
| **Automatic `Attached → Stale/Unreachable` transitions target `Attached` items only** | Only items in `Attached` state are candidates for automatic ownership transition by `detect_transitions::run()`. Items already in `Detached`, `Stale`, `Unreachable`, `Adopted`, or `Orphaned` state are left unchanged. This prevents re-transitioning already-resolved items if, for example, index corruption creates a duplicate `git_common_dir` entry for an already-adopted repo. |
| **Ownership state transitions are written in `repo repair`, not `repo status`** | `repo status` is a read-only command; writing to manifests inside a status call would violate Unix CLI conventions (scripts and CI pipelines assume status = read-only). `detect_transitions::run()` (write) is called from `repo repair`; `detect_transitions::scan()` (read-only) is called from `repo status` to surface a hint without side effects. |
| **Reclaim vs. transfer in `repo adopt`** | When `adopt` encounters an `Unreachable` item whose source repo shares the same `git_common_dir` as the current repo, it is treated as a **reclaim** (same logical repo, new identity) and the source item transitions to `Attached`. All other cases are **transfers**: the source item transitions to `Adopted`. Current heuristic: `git_common_dir` equality. Future ownership metadata (e.g. stable item lineage) may replace or refine this. |
| **`item relink` targets `Detached` items only** | `item relink` reverses `item restore --keep-store`. It verifies `ownership_state == Detached` before proceeding and transitions to `Attached`. It is not a substitute for `item repair`: `item repair` is ownership-neutral (fixes broken symlinks on `Attached` items without touching `ownership_state`); `item relink` changes ownership state and is only valid for `Detached` items. |

## Repair policy

The following table defines the contract for `shelfbox repair` depending on the state found at the repo-side path.

| State at repo-side path | `repair` behaviour |
|---|---|
| Missing (no file) | Recreate the symlink pointing to the store item. |
| Dangling symlink (target deleted) | Return `Err(RepairSymlinkTargetMismatch)` unless `force = true`; with force, remove and recreate. |
| Wrong-target symlink (points to a live file/dir outside the store) | Return `Err(RepairSymlinkTargetMismatch)` unless `force = true`; with force, remove and recreate. |
| Regular file (not a symlink) | Return `Err(PathIsRegularFile)` — refuse to overwrite user data. |
| Directory | Return `Err(PathIsRegularFile)` — refuse to overwrite. |
| Already healthy (correct symlink) | No-op, return `Ok(AlreadyHealthy)`. |
