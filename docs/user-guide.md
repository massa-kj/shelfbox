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

## Global flags

All subcommands accept one global flag:

| Flag | Description |
|---|---|
| `--store <PATH>` | Override the store directory for this invocation. Takes precedence over `config.toml`. |

---

## Command structure

Commands are grouped into five top-level groups:

```
shelfbox
├── item    — manage individual shelved items in the current repo
├── repo    — manage the current repository's shelf
├── store   — manage the global store
├── config  — manage shelfbox configuration
└── internal — development and debug utilities (hidden)
```

All output can be formatted with `--format <FORMAT>` where supported:

| Format | Description |
|---|---|
| `table` (default) | Human-readable aligned columns |
| `plain` | One item per line, machine-parseable |
| `json` | JSON output |

Use `--verbose` to show extended fields (store path, symlink target, all health
fields) in table format. The `default_format` config key sets the format used
when `--format` is not specified.

---

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

> **Caution: directory shelving**
> `item add` accepts directories, but shelving a directory that contains
> Git-tracked files or nested repositories is **not tested and not recommended**.
> Stick to shelving individual files.

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

**Outcomes reported:**

| Outcome | Meaning |
|---|---|
| `repaired` | Symlink was recreated successfully. |
| `ok (no repair needed)` | Symlink was already healthy. |
| `error: store item missing` | Store copy is gone — data may be lost. |
| `error: not managed` | Path is not recorded in the manifest. |

**Flags:**

| Flag | Description |
|---|---|
| `--dry-run` | Print what would happen without making any changes. |

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
```

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

**Flags:**

| Flag | Description |
|---|---|
| `--format <FORMAT>` | Output format: `table` (default), `plain` (store path only), `json`. |

---

## `repo` — manage the current repository's shelf

### `repo list`

Lists all repositories known to the store, with item counts.

```sh
shelfbox repo list
shelfbox repo list --format plain
shelfbox repo list --format json
shelfbox repo list --verbose
```

**Output (table, default):**

```
  NAME                           ROOT                                               ITEMS  LAST SEEN
  myapp                          /home/user/projects/myapp                              2  2026-04-29T12:00:00Z
```

**Flags:**

| Flag | Description |
|---|---|
| `--format <FORMAT>` | Output format: `table` (default), `plain`, `json`. |
| `--verbose` | Show extended fields: git_common dir, store_dir, and last_seen timestamp per repository. |

---

### `repo status`

Runs a full integrity check on the current repository's shelved items and
reports any problems (equivalent to the old `doctor` command).

```sh
shelfbox repo status
shelfbox repo status --format plain
shelfbox repo status --verbose
```

**Checks:**

- Per-item symlink and store-file health (same as `item status`).
- Orphan store items: files in the store not referenced by the manifest.
- Repo root match: verifies the recorded root path matches the current repo.

**Flags:**

| Flag | Description |
|---|---|
| `--format <FORMAT>` | Output format: `table` (default), `plain`, `json`. |
| `--verbose` | Show all health fields for each item individually. |

**Exit codes:**

| Code | Meaning |
|---|---|
| `0` | All items are healthy. |
| `1` | Warnings only (e.g. missing exclude entry). |
| `2` | Errors present (broken symlink, missing store item, git-tracked item). |

---

### `repo repair`

Applies safe automatic repairs to the current repository's shelf (equivalent
to the old `doctor --fix`).

```sh
shelfbox repo repair
shelfbox repo repair --dry-run
```

**What is fixed automatically:**

| Problem | Action |
|---|---|
| Index root mismatch | Updates recorded root to current path |
| Missing `.git/info/exclude` entries | Re-adds paths from manifest |
| Missing or broken symlinks | Recreates symlink |
| Store item missing | Reports WARN — cannot auto-fix |

**Flags:**

| Flag | Description |
|---|---|
| `--dry-run` | Print what would be fixed without making any changes. |

---

### `repo gc`

Deletes orphan store items (files in the store not referenced by the manifest).

```sh
shelfbox repo gc
shelfbox repo gc --dry-run
shelfbox repo gc --yes
```

**Flags:**

| Flag | Description |
|---|---|
| `--dry-run` | Print what would be deleted without making any changes. |
| `--yes` | Skip confirmation and perform deletions immediately. |

---

## `store` — manage the global store

### `store info`

Displays metadata about the global store.

```sh
shelfbox store info
```

**Output:**

```
Store path  : /home/user/.local/share/shelfbox
Repositories: 3
Total items : 7
Disk usage  : 12.3 KiB
```

---

### `store verify`

Runs a deep integrity check across all repos in the store, checking that every
manifest entry has a corresponding symlink and store file.

```sh
shelfbox store verify
```

Prints `MISS` lines for any problems found, then a summary.

**Exit codes:**

| Code | Meaning |
|---|---|
| `0` | No issues found. |
| `2` | One or more issues found. |

---

### `store gc`

Removes store entries for repositories whose root directory no longer exists
on disk (e.g. after deleting or moving a repository without restoring its
items first).

```sh
shelfbox store gc
shelfbox store gc --dry-run
shelfbox store gc --yes
```

**Flags:**

| Flag | Description |
|---|---|
| `--dry-run` | Print what would be deleted without making any changes. |
| `--yes` | Skip confirmation and perform deletions immediately. |

---

## `config` — manage configuration

### `config list`

Lists all configuration keys with their current values and origins.

```sh
shelfbox config list
shelfbox config list --format json
```

**Output (table, default):**

```
KEY             TYPE  DEFAULT                  SOURCE   CURRENT
store           path  ~/.local/share/shelfbox  default  ~/.local/share/shelfbox
default_format  enum  table                    default  table
```

**Flags:**

| Flag | Description |
|---|---|
| `--format <FORMAT>` | Output format: `table` (default), `json`. |

---

### `config path`

Prints the path to the active configuration file.

```sh
shelfbox config path
# /home/user/.config/shelfbox/config.toml
```

---

### `config get <KEY>`

Prints the resolved value of a configuration key. Always returns a value
(falls back to the built-in default if not configured).

```sh
shelfbox config get store
# /home/user/.local/share/shelfbox

shelfbox config get store --source
# /home/user/.local/share/shelfbox
# source: default
```

Supported keys: `store`, `default_format`.

**Flags:**

| Flag | Description |
|---|---|
| `--source` | Also print where the value comes from (`cli`, `env`, `config`, `default`). |

---

### `config set <KEY> <VALUE>`

Updates a configuration key in `config.toml` without touching other content
(comments and unknown keys are preserved). Creates the file if it does not
exist.

```sh
shelfbox config set store /mnt/external/shelfbox-store
shelfbox config set default_format json
```

Supported keys:

| Key | Values |
|---|---|
| `store` | Absolute path |
| `default_format` | `table`, `plain`, `json` |

---

### `config explain <KEY>`

Shows the type, default, description, and resolution precedence for a
configuration key.

```sh
shelfbox config explain store
shelfbox config explain default_format
```

---

## Configuration file

Optional TOML config file at:

| Platform | Default path |
|---|---|
| Linux / macOS | `$XDG_CONFIG_HOME/shelfbox/config.toml` → `~/.config/shelfbox/config.toml` |

```toml
# config.toml

# Absolute path to the store root directory.
# Default: $XDG_DATA_HOME/shelfbox (~/.local/share/shelfbox on Linux)
store = "/mnt/external/shelfbox-store"

# Default output format for list/status commands.
# Valid values: table (default), plain, json
# default_format = "table"
```

If the config file does not exist, all defaults are used silently.

### Environment variable

The `SHELFBOX_STORE` environment variable overrides the config file but is
overridden by the `--store` CLI flag.

Priority (highest → lowest):

| Source | Example |
|---|---|
| `--store` CLI flag | `shelfbox --store /tmp/my-store item list` |
| `$SHELFBOX_STORE` env var | `SHELFBOX_STORE=/work/store shelfbox item list` |
| `store` key in config.toml | `store = "/mnt/external/shelfbox-store"` |
| XDG / platform default | `~/.local/share/shelfbox` |

---

## Store layout

```
~/.local/share/shelfbox/
  meta.json                         # store identity (store_id ULID, created_at)
  index.json                        # maps ULID → repo metadata (root path, etc.)
  repos/
    api-server-01JTAR…/             # <sanitized-repo-name>-<ULID>
      manifest.json                 # items shelved from this repo
      items/
        .env                        # the actual shelved files
        secrets/
          api_key.txt
```

The store root and all repository subdirectories are created with mode `0700`
(owner-only read/write/execute) on Unix.

---

## Common workflows

### Keeping a `.env` file out of Git

```sh
echo "DATABASE_URL=postgres://…" > .env
shelfbox item add .env
# .env is now a symlink; your app still reads it normally
git status  # .env does not appear — it's in .git/info/exclude
```

### Moving the store to a different location

```sh
mv ~/.local/share/shelfbox /path/to/new/location
echo 'store = "/path/to/new/location"' > ~/.config/shelfbox/config.toml

# Or use the flag per-invocation
shelfbox --store /path/to/new/location item list
```

### Diagnosing problems after moving a repository

```sh
shelfbox repo status
# ERROR    repo root mismatch: repository may have been moved

shelfbox repo repair
# FIXED        updated repo root in index
```

### Recovering from a broken or missing symlink

```sh
shelfbox item status
# ERROR    .env  (symlink missing)

shelfbox item repair .env
# repaired: .env
```

### Recovering from a lost manifest

If `manifest.json` is accidentally deleted, `repo repair` rebuilds it from the
store's `items/` directory. The store path layout is deterministic
(`items/<repo-relative-path>`), so all items are recovered exactly.

### Shell completions

```sh
# Bash
shelfbox internal completions bash >> ~/.bash_completion

# Zsh
shelfbox internal completions zsh > ~/.zsh/completions/_shelfbox

# Fish
shelfbox internal completions fish > ~/.config/fish/completions/shelfbox.fish
```

### Debugging internal state

`internal debug` dumps the active configuration, store index, and current repo
context. By default, the home directory prefix in all paths is replaced with `~`
so the output is safe to paste into bug reports or AI chats.

```sh
shelfbox internal debug

# Print raw absolute paths (e.g. to share with a script that needs them)
shelfbox internal debug --allow-sensitive
```

### `doctor` alias

`shelfbox doctor` is a hidden alias for `shelfbox repo status`, following the
`brew doctor` / `flutter doctor` convention. It accepts the same `--format` flag.

```sh
shelfbox doctor
shelfbox doctor --format json
```
