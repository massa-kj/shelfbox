## `store` — manage the global store

### `store info`

Displays metadata about the global store.

```sh
shelfbox store info
```

**Output:**

```
Store path  : /home/user/.local/share/shelfbox
Store ID    : 01JTARXXXXXXXXXXXXXXXX
Hostname    : my-workstation
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

**Reclaimable items:**

Before deleting a repo's store directory, `store gc` loads its manifest and
checks for items in `attached`, `detached`, `stale`, or `unreachable` state.
If any exist, deletion is skipped even with `--yes`:

```
warning: skipping '<id>' [<name>]: 3 reclaimable item(s) — run 'shelfbox repo adopt --from <ID>' first
```

At the end of the run, skipped repos are counted separately:

```
Done. 2 removed, 1 skipped (reclaimable).
```

This guard is unconditional: `store gc` never force-deletes store files that
can still be recovered via `repo adopt`.
