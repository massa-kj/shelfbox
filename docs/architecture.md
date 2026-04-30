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
  config.rs       # Config load / XDG paths
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
    doctor.rs     # doctor() — full integrity report
```

### `shelfbox` (binary)

```
src/
  main.rs         # entry point — calls cli::run()
  cli.rs          # Clap struct + all cmd_* dispatch functions
```

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
      "last_seen_at": "2026-04-29T12:00:00Z"
    }
  }
}
```

The ULID key is stable for the lifetime of the repository entry.  `root` is an
absolute path; a mismatch between `root` and the current working Git root is
reported by `doctor` as a warning.

### Per-repository manifest — `manifest.json`

Stored at `<store>/repos/<ULID>/manifest.json`.

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
      "store_path": "/home/user/.local/share/shelfbox/repos/01JTAR…/items/.env",
      "kind": "File",
      "link": { "link_type": "Symlink" },
      "git": { "was_tracked": false },
      "created_at": "2026-04-29T12:00:00Z",
      "updated_at": "2026-04-29T12:00:00Z"
    }
  ]
}
```

`store_path` is an absolute path and is used during restore to locate the
store-side item without re-deriving it.

### Store directory tree

```
~/.local/share/shelfbox/
  index.json
  repos/
    <ULID>/
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

### `shelfbox add notes.md`

```
CLI parse (clap)
  └─ cli::cmd_add()
       ├─ context::build(cwd, store_override)
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

### `shelfbox doctor`

```
cli::cmd_doctor()
  ├─ context::build(…)
  ├─ ops::doctor::doctor(ctx, link, ignore)
  |    ├─ ops::status::status()                       // per-item checks
  |    ├─ collect_orphan_store_items()                // walk items/ dir
  |    └─ check_repo_root_in_index()                  // load index, compare root
  └─ print_doctor_report(statuses, orphans, root_matches)
```

---

## Design decisions

| Decision | Rationale |
|---|---|
| **No `git2` dependency** | `std::process::Command` is sufficient for the two queries needed (`rev-parse`, `ls-files`). Avoids a large C dependency and keeps compile times short. |
| **ULID for repo IDs** | Monotonically sortable, URL-safe, collision-resistant, and human-readable without a database. |
| **Atomic index writes** | `index.json` is written to a `.tmp` file and then renamed. `rename(2)` is atomic on POSIX; partial writes cannot corrupt the index. |
| **`thiserror` in lib / `anyhow` in bin** | Library errors should be typed and stable for callers; binary errors only need to be printable. Mixing them would pollute the library API. |
| **Paths in manifest are absolute** | Avoids re-deriving store paths on restore. `store_path` is canonical at write time. |
| **Exclude entries are sorted** | Deterministic ordering makes diffs human-readable and prevents spurious changes to `.git/info/exclude`. |
