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
5. Creates a symlink at the original location pointing to the store.
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

1. Validates that each path is a shelfbox managed symlink.
2. Checks that the store-side item exists.
3. Removes the symlink.
4. Moves the file back from the store to the repo.
5. Removes the item from `manifest.json`.
6. Removes the path from `.git/info/exclude` unless `--keep-ignore` is used.

**Flags:**

| Flag | Description |
|---|---|
| `--dry-run` | Print what would happen without making changes. |
| `--keep-ignore` | Do not remove the `.git/info/exclude` entry after restoring. |
| `--keep-store` | Keep the store copy and transition the item to `detached`. |

---

### `item repair <PATH>...`

Recreates a missing or broken symlink for one or more shelved files.

```sh
shelfbox item repair .env
shelfbox item repair secrets/api_key.txt
```

`item repair` is ownership-neutral. It does not touch the manifest state,
exclude entries, repository association, or store data.

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

Re-attaches a `detached` item by recreating its symlink and transitioning the
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
3. Refuses to overwrite a regular file at the repo path.
4. Recreates the symlink if needed.
5. Saves `ownership_state: attached`.

---

### `item move <OLD> <NEW>`

Renames a shelved item's tracked path without restoring and re-shelving it.

```sh
shelfbox item move .env .env.local
```

**What happens:**

1. Renames the store-side file.
2. Replaces the old symlink with a symlink at the new path.
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
