# User Guide

## Overview

`shelfbox` keeps files in your repository tree without letting Git see them.
It works by physically moving a file into an external **store** and replacing it
with a symlink, then adding the path to `.git/info/exclude` so Git ignores both
the symlink and any future file placed at that path.

Your editor, shell, and other tools follow the symlink transparently — the file
appears to be in its original location.

---

## Installation

```sh
cargo install --path crates/shelfbox
```

Requirements: Rust 1.75+, Git, Linux or macOS (symlinks required).

---

## Global flag

All subcommands accept one global flag:

| Flag | Description |
|---|---|
| `--store <PATH>` | Override the store directory for this invocation. Takes precedence over `config.toml`. |

---

## Commands

### `add <PATH>...`

Shelves one or more files or directories.

```sh
shelfbox add .env
shelfbox add secrets/notes/local.md
```

**What happens:**

1. Validates each path (see [Validation rules](#validation-rules)).
2. Moves the file/directory into `<store>/repos/<id>/items/<rel-path>`.
3. Creates a symlink at the original location pointing to the store.
4. Records the item in `manifest.json`.
5. Adds the repo-relative path to `.git/info/exclude`.

**Flags:**

| Flag | Description |
|---|---|
| `--dry-run` | Print what would happen without making any changes. |

**Validation rules** (checked in order):

1. Must be inside a Git repository.
2. Must be within the repository root.
3. Must not be inside `.git/`.
4. Must not be tracked by Git (`git ls-files --error-unmatch`).
5. Must not already be a symlink.
6. Must not already be managed by shelfbox.
7. The store destination must not already exist (no silent overwrites).

**Rollback:** if symlink creation fails after the move, the file is moved back
automatically.

### `restore <PATH>...`

Returns shelved files to their original locations.

```sh
shelfbox restore .env
shelfbox restore secrets/ notes/local.md
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
| `--keep-ignore` | Do not remove the `.git/info/exclude` entry after restoring. Useful when you plan to re-shelve the file shortly. |

**Rollback:** if the rename back to the repo fails, the symlink is recreated
automatically.

### `list`

Lists all files currently shelved in the current repository.

```sh
shelfbox list
shelfbox list --json
```

**Output (plain):**

```
  .env                                          file  2026-04-29T12:00:00Z
  secrets/api_key.txt                           file  2026-04-29T12:01:00Z
```

**Flags:**

| Flag | Description |
|---|---|
| `--json` | Emit a JSON array of manifest items. |

### `status`

Checks the health of every shelved item and reports problems.

```sh
shelfbox status
shelfbox status --json
```

**Output (plain):**

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
- `WARN` — `link_valid` and `store_exists` are true, but `in_exclude` or `not_tracked` is false.
- `ERROR` — `link_exists`, `link_valid`, or `store_exists` is false.

**Flags:**

| Flag | Description |
|---|---|
| `--json` | Emit JSON. |

### `doctor`

Runs all status checks plus deeper integrity checks.

```sh
shelfbox doctor
shelfbox doctor --json
```

**Additional checks beyond `status`:**

- **Orphan store items:** files inside the store's `items/` directory that are
  not referenced in the manifest (e.g. left by a previous version or manual
  intervention).
- **Repo root vs. index:** verifies that the path to this repository recorded
  in the global index matches the actual current path. A mismatch means the
  repository was moved or re-cloned.

**Output (plain):**

```
OK       repo root matches index
OK       .env
WARN     notes/scratch.md  (not in exclude)

--- orphan store items (not in manifest) ---
  WARN     orphan: stale_file.txt
```

**Flags:**

| Flag | Description |
|---|---|
| `--json` | Emit JSON (`DoctorReport`). |

---

## Configuration

Optional TOML config file at:

| Platform | Default path |
|---|---|
| Linux / macOS | `$XDG_CONFIG_HOME/shelfbox/config.toml` → `~/.config/shelfbox/config.toml` |

```toml
# config.toml

# Absolute path to the store root directory.
# Default: $XDG_DATA_HOME/shelfbox (~/.local/share/shelfbox on Linux)
store = "/mnt/external/shelfbox-store"
```

If the config file does not exist, all defaults are used silently.

---

## Store layout

```
~/.local/share/shelfbox/
  index.json                        # maps ULID → repo metadata (root path, etc.)
  repos/
    01JTAR…/                        # one directory per repository (ULID)
      manifest.json                 # items shelved from this repo
      items/
        .env                        # the actual shelved files
        secrets/
          api_key.txt
```

The store is designed to be **portable**: `manifest.json` records stable
metadata (remote URL, item kind, timestamps) and can be copied across machines.
Only `index.json` contains environment-specific absolute paths.

---

## Common workflows

### Keeping a `.env` file out of Git

```sh
echo "DATABASE_URL=postgres://…" > .env
shelfbox add .env
# .env is now a symlink; your app still reads it normally
git status  # .env does not appear — it's in .git/info/exclude
```

### Moving the store to a synced location (e.g. Dropbox)

```sh
# Move existing store
mv ~/.local/share/shelfbox /mnt/dropbox/shelfbox

# Tell shelfbox where it is
echo 'store = "/mnt/dropbox/shelfbox"' > ~/.config/shelfbox/config.toml

# Or use the flag per-invocation
shelfbox --store /mnt/dropbox/shelfbox list
```

### Diagnosing problems after moving a repository

```sh
shelfbox doctor
# ERROR    repo root mismatch: repository may have been moved
```

The mismatch is cosmetic — shelved files still work through the symlinks — but
it indicates that the index entry is stale.
