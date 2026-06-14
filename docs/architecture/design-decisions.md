# Design decisions

| Decision | Rationale |
|---|---|
| **No `git2` dependency** | `std::process::Command` is sufficient for the Git queries shelfbox needs. Avoiding `git2` keeps compile times shorter and avoids a large C dependency. |
| **ULID for repository and item IDs** | ULIDs are sortable, URL-safe, collision-resistant, and human-readable enough for CLI diagnostics without requiring a database. |
| **`repos/` is the canonical persistent store** | Preserving `repos/` must be enough to recover managed data. `index.json` and `meta.json` are local metadata/cache files and must be safely regenerable. |
| **`RepoId` is the only repository identity** | Repository store directory names, Git remotes, Git paths, and hostnames are not identity. This keeps recovery explicit and avoids accidental merges between clones. |
| **Human-readable repo store directories** | New repository store directories use `<name>`, then `<name>-2`, `<name>-3`, etc. The name is only a locator; the stable identity is the `repo_id` in `manifest.json`. |
| **Atomic index and manifest writes** | `index.json` and `manifest.json` are written with a temp-file-then-rename strategy so a crash mid-write cannot leave partial JSON. |
| **`store_path` is repo-store-relative** | Relative paths such as `items/.env` keep `manifest.json` portable across machines and store relocations. |
| **No absolute paths in `manifest.json`** | Absolute paths are local machine cache data. They belong in `index.json` and may be absent after `store rebuild-index`. |
| **Identity hints are hints only** | `remote_hints`, `repo_name_hints`, and `last_attached_at` are used for display, candidate ranking, and best-effort reclaim hints. They are never proof of identity and never trigger automatic reclaim. |
| **Remote hints use host/path format** | Remote URLs are normalized to `host/org/repo` style before entering `remote_hints` (for example `github.com/org/repo`). Scheme, user, query, fragment, and `.git` suffixes are discarded; local/file URLs are not stored as remote hints. |
| **Explicit reclaim instead of automatic identity detection** | A new clone receives a new `RepoId` unless the user explicitly runs `repo reclaim`. This prevents remote URL or name matches from silently merging unrelated repositories. |
| **Reclaim does not transfer ownership** | `repo reclaim` associates the current clone with an existing `RepoId`; it does not move items, copy items, change ownership state, repair symlinks, or rewrite excludes. |
| **Repair is ownership-neutral** | `repo repair` and `item repair` restore local integration but never assign a different `RepoId` or change item ownership state. |
| **Conservative GC** | GC may delete only confirmed `orphaned` items. `attached`, `detached`, and `unreachable` items are protected. Manifest entries are removed and saved before store files are deleted, so a manifest-save failure does not remove data. Repository store directories are not deleted merely because a local clone is missing. |
| **Namespace is UI only** | Directory grouping is derived from `item.path`. Namespace entries are not persisted as identity, ownership, recovery, reclaim, repair, or GC metadata. |
| **Link strategy is runtime behavior** | `LinkStrategy` selects how links are created on the current platform. Per-item link metadata is not canonical manifest identity. If future hardlink/copy modes need persistence, they require a new versioned manifest field and migration. |
| **`# BEGIN shelfbox` block in exclude** | All shelfbox entries are wrapped in a named block so other tools can safely edit `.git/info/exclude`; content outside the block is preserved. |
| **Store-level advisory file lock** | Repo-context operations acquire `<store>/.lock` so ordinary item and repository writes do not interleave index and manifest updates. |
| **Machine-readable exit codes** | Status and verify commands return stable process codes so they can be used in scripts and CI. |
| **Explicit manifest migration** | Legacy manifests are upgraded only by `store migrate-manifests`; normal commands reject unsupported versions instead of silently rewriting canonical data. |
| **`SHELFBOX_STORE` environment variable** | The store root can be selected by environment variable, with precedence below `--store` and above config/default paths. |
