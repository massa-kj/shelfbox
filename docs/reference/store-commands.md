## `store` — manage the global store

### `store info`

Displays metadata about the global store.

```sh
shelfbox store info
```

**Output:**

```text
Store path  : /home/user/.local/share/shelfbox
Store ID    : 01JTARXXXXXXXXXXXXXXXX
Hostname    : my-workstation
Repositories: 3
Total items : 7
Disk usage  : 12.3 KiB
```

---

### `store verify`

Runs an integrity check across the store.

For repositories with current local Git metadata, `store verify` checks both
repo-side symlinks and store files. For repositories rebuilt from manifests
alone (`root: None`), it verifies store-side data and reports that repo-side
checks require `repo reclaim` and `repo repair`.

```sh
shelfbox store verify
```

**Exit codes:**

| Code | Meaning |
|---|---|
| `0` | No issues found. |
| `2` | One or more issues found. |

---

### `store rebuild-index`

Regenerates `index.json` from canonical manifests under `repos/`.

```sh
shelfbox store rebuild-index
shelfbox store rebuild-index --dry-run
```

**Behavior:**

1. Scans `repos/*/manifest.json`.
2. Reads each valid `repo_id` and repository store directory.
3. Fails without writing if duplicate `repo_id` or duplicate `item_id` values exist.
4. Reports corrupted manifests and skips them.
5. Writes a new `index.json`.

Rebuilt entries contain:

```text
repo_id
repo_store_dir
last_seen_at
```

Rebuilt entries do not invent:

```text
root
git_dir
git_common_dir
```

These local Git metadata fields are restored later by `repo reclaim`, `repo
repair`, or normal repository operations.

---

### `store migrate-manifests`

Explicitly migrates legacy manifests to the current manifest schema.

```sh
shelfbox store migrate-manifests
shelfbox store migrate-manifests --dry-run
```

Migration is never performed automatically during normal command execution.

**Dry-run output includes:**

* Number of legacy manifests that would be converted
* Number of current manifests that would be left unchanged
* Manifests skipped or failed, with reasons
* Ownership-state mapping counts
* Namespace entries that would be dropped

The migration fails without writing if duplicate repository or item identities
are detected.

---

### `store gc`

Performs conservative garbage collection.

```sh
shelfbox store gc
shelfbox store gc --dry-run
```

**Restriction:**

`store gc` may delete only items whose manifest state is:

```text
orphaned
```

It must not delete:

```text
attached
detached
unreachable
```

It must not delete an entire repository store directory just because no current
Git clone is associated with that `RepoId`.

**Index reachability rules:**

* `root: None` means unassociated or rebuilt from manifests; it is normal after
  `store rebuild-index` and is not a deletion signal.
* `root: Some(path)` where `path` no longer exists means the local clone is not
  reachable on this machine.
* Even when the local clone is unreachable, only confirmed `orphaned` items may
  be deleted.

**Flags:**

| Flag | Description |
|---|---|
| `--dry-run` | Print what would be deleted without prompting or writing. |
