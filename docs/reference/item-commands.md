## `item` — manage shelved items

### `item add <PATH>...`

Shelves one or more files or directories.

```sh
shelfbox item add .env
shelfbox item add secrets/notes/local.md
```

**What happens:**

1. Validates each path (see [Validation rules](#validation-rules)).
2. Moves the file/directory into `<store>/repos/<id>/items/<rel-path>`.
3. Creates a symlink at the original location pointing to the store.
4. Records the item in `manifest.json`.
5. Adds the repo-relative path to `.git/info/exclude` inside a managed block:
   ```
   # BEGIN shelfbox
   .env
   # END shelfbox
   ```
   The block is replaced atomically on every write. Lines outside the block
   are never touched.

**Flags:**

| Flag | Description |
|---|---|
| `--dry-run` | Print what would happen without making any changes. |

**Validation rules** (checked in order):

1. Must be inside a Git repository.
2. Must be within the repository root.
3. Must not be inside `.git/`.
4. Must not be tracked by Git (`git ls-files --error-unmatch`).
   If the file is tracked, shelfbox prints a hint:
   ```
   error: '.env' is tracked by git
   hint: remove it from the index first:
     git rm --cached .env
   then re-run: shelfbox item add .env
   ```
5. Must not already be a symlink.
6. Must not already be managed by shelfbox.
7. The store destination must not already exist (no silent overwrites).

**Rollback:** if symlink creation fails after the move, the file is moved back
automatically.

**Directory shelving:**

Pass a directory path to shelve all files inside it as a named **namespace**:

```sh
shelfbox item add secrets/
```

Each file under `secrets/` is shelved individually using the same rules as
single-file add:

- Git-tracked files, existing symlinks, and already-managed files are skipped
  with a reason.
- Nested git repositories are not entered — their contents are excluded and
  reported as errors.
- Partial success is allowed: if at least one file is shelved a namespace entry
  is created.

A summary is printed on completion:

```
namespace 'secrets/': 2 added, 0 skipped, 0 failed
  shelved: secrets/api_key.txt
  shelved: secrets/db_pass.txt
namespace registered: secrets/
```

**Namespace rules:**

- A namespace groups items in `item list` output but does not own them — each
  file remains independently repairable.
- Single-file `item add` for a file inside a namespace directory does **not**
  create a new namespace entry.
- After all files in a namespace are restored, the namespace entry is removed
  automatically.
- Namespace entries are not recovered by `repo repair`. Re-register by
  re-running `item add <dir>/` (the files are still managed; only the grouping
  entry is recreated).

---

### `item restore <PATH>...`

Returns shelved files to their original locations.

```sh
shelfbox item restore .env
shelfbox item restore secrets/ notes/local.md
```

**What happens:**

1. Validates that each path is a shelfbox managed symlink.
2. Checks that the store-side item exists (guards against dangling links).
3. Removes the symlink.
4. Moves the file/directory back from the store to the repo.
5. Removes the item from `manifest.json`.
6. Removes the path from `.git/info/exclude` (unless `--keep-ignore`).

**Flags:**

| Flag | Description |
|---|---|
| `--dry-run` | Print what would happen without making any changes. |
| `--keep-ignore` | Do not remove the `.git/info/exclude` entry after restoring. |
| `--keep-store` | Remove the item from the manifest only. The symlink and the store file are left in place (turns the store item into an orphan). Useful for temporarily detaching an item without losing the store copy. |

**Errors:**

| Error | Meaning |
|---|---|
| `not a shelfbox managed symlink` | The path is not a symlink pointing into the shelfbox store. |
| `restore destination already exists as a regular file or directory` | A non-symlink entry exists at the path. Move or rename it first. |
| `store item not found` | The store-side copy is missing (dangling link). |

**Namespace restore:**

Pass a directory path (with or without a trailing `/`) to restore all files in a
namespace at once:

```sh
shelfbox item restore secrets/
```

1. Finds all manifest items whose path starts with `secrets/`.
2. Restores each one individually (same semantics as single-file restore).
3. Removes the `secrets/` namespace entry automatically after the last member
   is restored.

A summary is printed on completion:

```
namespace 'secrets/': 2 restored, 0 failed
  restored: secrets/api_key.txt
  restored: secrets/db_pass.txt
namespace removed: secrets/
```

**Additional error:**

| Error | Meaning |
|---|---|
| `no namespace registered for 'secrets/'` | The path is not registered as a namespace. Run `item add secrets/` first. |

---

### `item repair <PATH>...`

Recreates a missing or broken symlink for one or more shelved files.

```sh
shelfbox item repair .env
shelfbox item repair secrets/api_key.txt .env.local
```

Use `item repair` when `item status` shows `symlink missing` or `symlink invalid`
for a file whose store-side copy still exists. It does not touch the manifest,
exclude entries, or the store itself — it only fixes the symlink.

**Wrong-target symlinks require `--force`:**

If a symlink exists at the path but points to an unexpected location (for example,
a stale link left after a reclone, store relocation, or copied repo), `repair`
refuses to overwrite it without `--force`. This prevents silently masking a
potentially incorrect machine or store state.

```sh
# Without --force: error
shelfbox item repair .env
# error: symlink target mismatch at '.env': points to '/old/store/.env',
#        expected '/current/store/.env'
# hint: run 'shelfbox item repair --force' if this is intentional

# Investigate the discrepancy, then override explicitly if correct
shelfbox item repair --force .env
```

**Outcomes reported:**

| Outcome | Meaning |
|---|---|
| `repaired` | Symlink was recreated successfully. |
| `ok (no repair needed)` | Symlink was already healthy. |
| `error: symlink target mismatch` | A symlink exists but points elsewhere. Use `--force` to override. |
| `error: store item missing` | Store copy is gone — data may be lost. |
| `error: not managed` | Path is not recorded in the manifest. |

**Flags:**

| Flag | Description |
|---|---|
| `--dry-run` | Print what would happen without making any changes. |
| `--force` | Allow overwriting a wrong-target symlink. Without this flag, `repair` refuses to touch symlinks that point to an unexpected location. |

---

### `item relink <PATH>...`

Re-attaches a `detached` item by recreating its symlink and transitioning the
manifest state from `detached` to `attached`.

```sh
shelfbox item relink .env
shelfbox item relink secrets/api_key.txt .env.local
```

A `detached` item is created by `item restore --keep-store`: the store file is
preserved and the manifest entry is updated to `ownership_state: detached`.
`item relink` reverses this without requiring a full `item restore → item add`
cycle.

**What happens:**

1. Looks up the item in the manifest and verifies `ownership_state == detached`.
2. Verifies the store file exists.
3. Checks that no regular file exists at the repo path (refuses to overwrite).
4. Recreates the symlink at the repo path if not already present and correct.
5. Transitions `ownership_state: detached → attached` and saves the manifest.

**Flags:**

| Flag | Description |
|---|---|
| `--dry-run` | Print what would happen without making any changes. |

**Errors:**

| Error | Meaning |
|---|---|
| `not a detached item` | The item's `ownership_state` is not `detached`. Use `item repair` for broken symlinks on `attached` items. |
| `store item not found` | The store-side copy is missing — run `item restore` and `item add` instead. |
| `path is occupied by a regular file` | A non-symlink file exists at the repo path. Move it first. |

---

### `item move <OLD> <NEW>`

Renames a shelved item's tracked path without restoring and re-shelving it.

Both `OLD` and `NEW` are paths relative to the repository root.

```sh
shelfbox item move .env .env.local
shelfbox item move secrets/api_key.txt secrets/keys/api_key.txt
```

**What happens:**

1. The store-side file is renamed atomically (`items/<old>` → `items/<new>`).
2. The old symlink is removed and a new symlink is created at `NEW`.
3. The manifest is updated with the new path and store path.
4. `.git/info/exclude` is updated: old entry removed, new entry added.

**Flags:**

| Flag | Description |
|---|---|
| `--dry-run` | Print what would happen without making any changes. |

**Errors:**

| Error | Meaning |
|---|---|
| `not a shelfbox managed symlink` | `OLD` is not recorded in the manifest. |
| `move destination already exists` | A file or directory already exists at `NEW`. |
| `already managed by shelfbox` | `NEW` is already a shelved path. |
| `symlink does not point to expected store location` | The symlink at `OLD` is inconsistent. Run `item repair` first. |
| `moving directory items is not supported` | `OLD` is a shelved directory — not supported in this version. |

> **Note:** Moving directory items is not supported in this version.
> As a workaround: `item restore` the directory, rename it with `mv`, then `item add` the new path.

---

### `item list`

Lists all files currently shelved in the current repository.

```sh
shelfbox item list
shelfbox item list --format plain
shelfbox item list --format json
shelfbox item list --verbose
```

**Output (table, default):**

```
  PATH                                          KIND  CREATED
  .env                                          file  2026-04-29T12:00:00Z
  secrets/api_key.txt                           file  2026-04-29T12:01:00Z
  secrets/db_pass.txt                           file  2026-04-29T12:01:00Z
```

Items that belong to a namespace are listed at their normal path alongside
non-namespace items.  Use `--format json` to see the raw `namespaces` array
from the manifest.

**Flags:**

| Flag | Description |
|---|---|
| `--format <FORMAT>` | Output format: `table` (default), `plain`, `json`. |
| `--verbose` | Show extended fields: store path and symlink target for each item. |

---

### `item status`

Checks the health of every shelved item and reports problems.

```sh
shelfbox item status
shelfbox item status --format json
shelfbox item status --verbose
```

**Output (table, default):**

```
OK       .env
WARN     notes/scratch.md  (not in exclude)
ERROR    secrets/db.env    (symlink missing, store item missing)
```

Each item is checked for:

| Check | Meaning |
|---|---|
| `link_exists` | A filesystem entry exists at the repo-side path. |
| `link_valid` | The entry is a symlink pointing into the store. |
| `store_exists` | The store-side file/directory exists. |
| `in_exclude` | The path appears in `.git/info/exclude`. |
| `not_tracked` | The path is not tracked by Git. |

Severity:
- `OK` — all checks pass.
- `WARN` — symlink and store are healthy, Git is not tracking the file, but the path is missing from `.git/info/exclude`.
- `ERROR` — any of: symlink missing, symlink invalid, store item missing, or Git is tracking the file (`not_tracked` is false).

**Flags:**

| Flag | Description |
|---|---|
| `--format <FORMAT>` | Output format: `table` (default), `plain`, `json`. |
| `--verbose` | Show all health fields (link_exists, link_valid, store_exists, in_exclude, not_tracked) for each item. |

**Exit codes:**

| Code | Meaning |
|---|---|
| `0` | All items OK. |
| `1` | At least one WARN (path missing from `.git/info/exclude`). |
| `2` | At least one ERROR (symlink missing, symlink invalid, store item missing, or path tracked by Git). |

Suitable for use in shell scripts:

```sh
shelfbox item status || echo "issues detected (exit $?)"
```

---

### `item info <PATH>`

Displays detailed metadata about a single shelved item.

```sh
shelfbox item info .env
shelfbox item info .env --format json
```

**Output (table, default):**

```
path        .env
repo_root   ~/projects/myapp
store_path  ~/.local/share/shelfbox/repos/myapp-01JT…/items/.env
link_target ~/.local/share/shelfbox/repos/myapp-01JT…/items/.env
symlink_ok  true
tracked     true
in_exclude  true
```

Ownership metadata (`item_id`, `origin_repo_id`, `ownership_state`) is
available via `item list --format json`.

**Flags:**

| Flag | Description |
|---|---|
| `--format <FORMAT>` | Output format: `table` (default), `plain` (store path only), `json`. |
