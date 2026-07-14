## `item` — manage shelved items

### `item add <PATH>...`

Shelves one or more files.

```sh
shelfbox item add .env
shelfbox item add secrets/notes/local.md
```

**What happens:**

1. Resolves the current Git repository.
2. Reuses the existing `RepoId` when `index.json` matches by `root` or
   `git_common_dir`.
3. Creates a new `RepoId` only when no local cache match exists.
4. Moves the file into `<store>/repos/<repo-store-dir>/items/<rel-path>`.
5. Creates the configured materialization at the original location: a symlink
   by default, or an independent regular copy in Copy mode.
6. Records the item in `manifest.json`.
7. Adds the repo-relative path to `.git/info/exclude` inside a managed block.
8. Updates `identity_hints` with normalized remote and repository-name hints.

Repository resolution for `item add` must not use manifest hints as identity
proof and must not automatically reclaim an existing `RepoId`.

If no local cache entry matches but existing manifests contain positive reclaim
candidates, the CLI prints a hint to run `shelfbox repo reclaim`. It still keeps
reclaim explicit and does not select or attach a candidate automatically.

**Flags:**

| Flag | Description |
|---|---|
| `--dry-run` | Print what would happen without making changes. |

**Validation rules:**

1. Must be inside a Git repository.
2. Must be within the repository root.
3. Must not be inside `.git/`.
4. Must not be tracked by Git.
5. Must not already be a symlink.
6. Must not already be managed by shelfbox.
7. The store destination must not already exist.

**Directory paths:**

When directory shelving is supported, each file under the directory is still
represented as an independent item. Directory grouping is UI presentation
derived from `item.path`; no namespace entry is persisted in `manifest.json`.

---

### `item restore <PATH>...`

Returns shelved files to their original locations.

```sh
shelfbox item restore .env
shelfbox item restore secrets/notes/local.md
```

**What happens:**

1. Validates that each path is a managed symlink or an equal, isolated regular
   copy.
2. Checks that the store-side item exists.
3. Removes the materialization when needed.
4. Moves the file back from the store to the repo.
5. Removes the item from `manifest.json`.
6. Removes the path from `.git/info/exclude` unless `--keep-ignore` is used.

An equal regular copy is retained while management is removed. A diverged copy
is never overwritten or deleted by restore; run `item sync --from store` or
`item sync --from repo --yes` first.

**Flags:**

| Flag | Description |
|---|---|
| `--dry-run` | Print what would happen without making changes. |
| `--keep-ignore` | Do not remove the `.git/info/exclude` entry after restoring. |
| `--keep-store` | Detach: retain the observed materialization, store item, manifest entry, and exclude, then transition to `detached`. |

---

### `item repair <PATH>...`

Recreates a missing materialization for one or more shelved files.

```sh
shelfbox item repair .env
shelfbox item repair secrets/api_key.txt
```

`item repair` is ownership-neutral. It does not touch the manifest state,
exclude entries, repository association, or store data.

For a missing entry, repair uses the configured strategy. Equal regular copies
and valid symlinks are no-ops; a diverged regular copy is reported and left
unchanged until an explicit sync is chosen.

**Wrong-target symlinks require `--force`:**

If a symlink exists at the path but points to an unexpected location, `repair`
refuses to overwrite it without `--force`.

**Flags:**

| Flag | Description |
|---|---|
| `--dry-run` | Print what would happen without making changes. |
| `--force` | Allow overwriting a wrong-target symlink. |

---

### `item relink <PATH>...`

Re-attaches a `detached` item by preserving or recreating its materialization and transitioning the
manifest state from `detached` to `attached`.

```sh
shelfbox item relink .env
```

A `detached` item is created by:

```sh
shelfbox item restore --keep-store <PATH>
```

**What happens:**

1. Verifies `ownership_state == detached`.
2. Verifies the store file exists.
3. Preserves a healthy symlink or equal regular copy, or recreates a missing
   materialization with the configured strategy.
4. Refuses to overwrite an unexpected regular file.
5. Saves `ownership_state: attached`.

A diverged detached regular copy requires an explicit direction:

```sh
shelfbox item relink .env --from store
shelfbox item relink .env --from repo --yes
```

`--from repo` requires `--yes` for an actual write because it replaces the
canonical store content. `--dry-run` does not require confirmation.

---

### `item sync <PATH>... --from <store|repo>`

Synchronizes a diverged regular copy in an explicit direction. It never
chooses a source automatically.

```sh
shelfbox item sync .env --from store
shelfbox item sync .env --from repo --yes
```

`--from store` replaces the repository copy with canonical content. `--from
repo` replaces canonical store content and requires `--yes` except for
`--dry-run`. Managed symlinks and already-equal copies are no-ops. The command
rejects missing, tracked, hardlinked, or exclude-missing copies rather than
silently changing content.

| Flag | Description |
|---|---|
| `--from <store|repo>` | Required source of truth for this invocation. |
| `--dry-run` | Print the approved action without writing. |
| `--yes` | Required with `--from repo` when not dry-running. |

---

### `item move <OLD> <NEW>`

Renames a shelved item's tracked path without restoring and re-shelving it.

```sh
shelfbox item move .env .env.local
```

**What happens:**

1. Renames the store-side file.
2. Moves the observed materialization to the new path without converting its
   strategy.
3. Updates `path`, `store_path`, and `updated_at` in the manifest.
4. Updates `.git/info/exclude`.

Directory grouping is derived from paths, so moving an item may change how a UI
groups it without changing ownership.

---

### `item list`

Lists files currently shelved in the current repository.

```sh
shelfbox item list
shelfbox item list --format plain
shelfbox item list --format json
shelfbox item list --verbose
```

**Output (table, default):**

```text
  PATH                                          STATE       CREATED
  .env                                          attached    2026-04-29T12:00:00Z
  secrets/api_key.txt                           attached    2026-04-29T12:01:00Z
```

Directory grouping, if shown by a future UI, is derived from `path`; no
namespace metadata is stored.

---

### `item status`

Checks the health of every shelved item and reports problems.

```sh
shelfbox item status
shelfbox item status --format json
shelfbox item status --verbose
```

Each item is checked for:

| Check | Meaning |
|---|---|
| `link_exists` | A filesystem entry exists at the repo-side path. |
| `link_valid` | The entry is a symlink pointing into the store. |
| `store_exists` | The store-side file exists. |
| `in_exclude` | The path appears in `.git/info/exclude`. |
| `not_tracked` | The path is not tracked by Git. |

JSON status uses schema version 2. In addition to the legacy link fields it
reports `configured_strategy`, `observed_materialization`,
`materialization_exists`, `materialization_valid`, `content_state`, `severity`,
stable `issues`, and informational `notes`. For regular-copy items,
`link_exists` and `link_valid` are `null`; consumers should use the generic
materialization fields. A diverged copy is an error with
`content_diverged`, while a strategy mismatch alone is only a note.

---

### `item info <PATH>`

Displays detailed metadata about a single shelved item.

```sh
shelfbox item info .env
shelfbox item info .env --format json
```

Ownership metadata (`item_id`, `origin_repo_id`, `ownership_state`) is available
via JSON output.

**Flags:**

| Flag | Description |
|---|---|
| `--format <FORMAT>` | Output format: `table` (default), `plain`, or `json`. |
