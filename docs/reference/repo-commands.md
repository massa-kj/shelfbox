## `repo` — manage the current repository's shelf

### `repo list`

Lists repositories known to the store.

```sh
shelfbox repo list
shelfbox repo list --format plain
shelfbox repo list --format json
shelfbox repo list --verbose
```

Entries rebuilt from manifests may not have local Git metadata. In that case,
the repository is shown as unassociated until `repo reclaim` or `repo repair`
refreshes the index entry.

**Flags:**

| Flag | Description |
|---|---|
| `--format <FORMAT>` | Output format: `table` (default), `plain`, `json`. |
| `--verbose` | Show extended fields such as repo store directory, Git metadata if present, and last-seen timestamp. |

---

### `repo status`

Runs a read-only health check on the current repository's shelved items.

```sh
shelfbox repo status
shelfbox repo status --format plain
shelfbox repo status --verbose
```

**Checks:**

* Per-item symlink and store-file health
* Git exclude entries
* Whether the current repository is associated with a known `RepoId`

`repo status` is read-only. It does not perform reclaim, repair symlinks, or
mutate manifests.

If the current clone has no local cache match but manifests contain positive
reclaim candidates, `repo status` prints a hint to run `shelfbox repo reclaim`.
The hint is informational only; reclaim remains an explicit user action.

**Exit codes:**

| Code | Meaning |
|---|---|
| `0` | All items are healthy. |
| `1` | Warnings only. |
| `2` | Errors present. |

---

### `repo reclaim`

Associates the current Git clone with an existing `RepoId`.

```sh
shelfbox repo reclaim
shelfbox repo reclaim --repo-id 01ABC...
```

Use this after restoring `repos/`, rebuilding `index.json`, moving to a new PC,
or re-cloning a repository.

**Preconditions:**

* The current directory must be inside a Git repository.
* If the current repository is already associated with shelfbox, it must have no managed items.

**Behavior:**

1. Detects current Git metadata without creating a new `RepoId`.
2. Scans `repos/*/manifest.json`.
3. Builds reclaim candidates from manifests and hints.
4. Displays candidates for explicit user selection, unless `--repo-id` is used.
5. Updates `index.json` for the selected `RepoId`.
6. Updates `identity_hints`.

Without `--repo-id`, choose a displayed candidate by number or enter `q` to quit without changes. If no candidates are found, the command exits without writing.

With `--repo-id`, shelfbox skips interactive selection and reclaims that identity directly after the same validation checks.

Successful output:

```text
Associated with <repo_id>. Run `shelfbox repo repair` to restore symlinks.
```

`repo reclaim` does not:

* Move or copy item data
* Change item ownership state
* Repair symlinks
* Rewrite Git exclude entries

After reclaim, run:

```sh
shelfbox repo repair
```

---

### `repo repair`

Repairs local working tree integration for an already-associated repository.

```sh
shelfbox repo repair
shelfbox repo repair --dry-run
shelfbox repo repair --force
```

If the current clone is not associated with a `RepoId`, run `repo reclaim`
first.

**What is fixed:**

| Problem | Action |
|---|---|
| Missing or broken symlinks for `attached` items | Recreates symlinks |
| Missing or stale `.git/info/exclude` entries | Rebuilds shelfbox exclude block from `attached` items |
| Stale local Git metadata in `index.json` | Updates `root`, `git_dir`, and `git_common_dir` |
| Missing identity hints | Updates repo-name, remote, and last-attached hints |
| Missing store item | Reports failure for that item |

Wrong-target symlinks are reported as per-item failures by default. Use
`--force` only after checking that replacing those symlinks is intentional.

Successful output is summarized as:

```text
repo repair:
  symlinks repaired: <count>
  symlinks already healthy: <count>
  symlinks failed: <count>
  exclude: updated|already current
  index: updated|already current
  identity hints: updated|already current
```

With `--dry-run`, the changed metadata lines use `would update`, and symlink
counts report what would be repaired without writing files.

`repo repair` must not:

* Perform reclaim
* Assign a different `RepoId`
* Transfer ownership
* Change item ownership state
* Delete item data

**Flags:**

| Flag | Description |
|---|---|
| `--dry-run` | Print what would be fixed without making changes. |
| `--force` | Replace wrong-target symlinks instead of reporting them as failures. |

---

### `repo gc`

`repo gc` is retained only for current-repository orphan inspection. It lists
store files under the current repo's `items/` directory that are not referenced
by the manifest, but it does not delete them. Store-wide deletion rules live
under `store gc`.

Garbage collection must follow the ownership model:

* Only `orphaned` items may be deleted.
* `attached`, `detached`, and `unreachable` items are protected.
* Repository store directories are not deleted merely because a local clone is
  missing.
* `repo gc --yes` is ignored for deletion; use `store gc --yes` after items are
  explicitly marked `orphaned`.
