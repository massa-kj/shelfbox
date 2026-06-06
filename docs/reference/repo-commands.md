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

**Ownership transition hint:**

If other repositories in the same store have `attached` items that would be
affected by the current repo's presence (e.g. same `git_common_dir` after a
reclone), `repo status` prints a hint to `stderr`:

```
hint: N item(s) in M repo(s) may need ownership transition — run 'shelfbox repo repair' to apply
```

This check is read-only: no manifests are modified by `repo status`.

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
| `attached` items in other repos superseded by this repo | Transitions to `stale` or `unreachable` (see below) |

**Ownership state detection:**

`repo repair` automatically scans all other repos in the store and transitions
`attached` items that are no longer current:

- **`attached → stale`**: another repo entry shares the same `git_common_dir`
  as the current repo (e.g. after a reclone that generated a new ULID). Old
  items become reclaimable via `repo adopt --from <OLD-ID>`.
- **`attached → unreachable`**: a repo's root directory no longer exists on
  disk (repo deleted or moved without re-registering).

Only `attached` items are ever auto-transitioned. Items already in `detached`,
`stale`, `unreachable`, `adopted`, or `orphaned` state are left unchanged.

This detection runs before the integrity fix pass so that subsequent status
checks reflect up-to-date ownership information.  It is skipped in dry-run mode.

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

**Ownership-protected items:**

Before listing FS orphan candidates, `repo gc` checks the current repo's
manifest for items in `detached`, `stale`, or `unreachable` state. These items
are not FS orphans (they remain in the manifest) but are reported separately:

```
ownership-protected items (not collected by gc):
  2 detached    — run 'shelfbox item relink <PATH>' to re-attach
  1 stale       — run 'shelfbox repo adopt --from <OLD-REPO-ID>' to reclaim
  1 unreachable — run 'shelfbox repo adopt --from <OLD-REPO-ID>' or 'shelfbox repo repair' to recover
```

---

### `repo adopt`

Transfers ownership of shelved items from a previous repository identity into
the current one.

Use this after a reclone, repository move, or path migration where the old
store entry is no longer reachable under the new repository identity.

```sh
# Find the old repository ID
shelfbox repo list --verbose

# Transfer its items to the current repo
shelfbox repo adopt --from 01JTARXXXXXXXXXXXXXXXX
shelfbox repo adopt --from 01JTARXXXXXXXXXXXXXXXX --dry-run
```

**What happens:**

1. Locates the source repository by its ID in the store index.
2. For each eligible item in the source manifest:
   - Copies the store file into the current repository's store directory.
   - Creates a symlink at the repo-relative path.
   - Records the item in the current manifest with `ownership_state: adopted`.
3. Marks the transferred items in the source manifest with `ownership_state: adopted`.
4. Saves both manifests atomically.

Items that conflict with an existing path in the current manifest are skipped.
Items whose store file is missing are also skipped.

**Flags:**

| Flag | Description |
|---|---|
| `--from <REPO_ID>` | Source repository ID to adopt items from. Required. |
| `--dry-run` | Print what would happen without making any changes. |

**Outcomes per item:**

| Outcome | Meaning |
|---|---|
| `adopted` | Item transferred and symlink created. |
| `adopted (no link)` | Item transferred but symlink creation failed. Run `item repair` to fix. |
| `reclaimed` | The source item was `unreachable` and shares the same `git_common_dir` as the current repo — treated as a reclaim (same logical repo, new identity). The source item transitions to `attached` rather than `adopted`. |
| `reclaimed (no link)` | Same as `reclaimed` but symlink creation failed. Run `item repair` to fix. |
| `skipped (conflict)` | Current manifest already contains an item at this path. |
| `skipped (store missing)` | Source store file not found. |

**Errors:**

| Error | Meaning |
|---|---|
| `cannot adopt from self` | `--from` refers to the current repository. |
| `no store entry found for repo id` | The ID is not in the store. Run `repo list --verbose` to see known IDs. |
