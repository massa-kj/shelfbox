# Failure Matrix

This document catalogues important failure and recovery scenarios in
`shelfbox`.

The guiding hierarchy is:

```text
Do not break data
  ↓
If broken, detect it
  ↓
If detected, recover conservatively
  ↓
Identity-changing actions require explicit user intent
```

---

## Failure Modes

| # | Scenario | How it breaks | Detection | Recovery command | Notes |
|---|---|---|---|---|---|
| 1 | Symlink deleted | Repo-side symlink gone; store file intact | `item status` / `repo status` | `item repair <PATH>` or `repo repair` | Store file unaffected |
| 2 | Store item deleted | Store file gone; manifest entry remains | `item status` / `repo status` | Manual recovery | Missing data cannot be recreated |
| 3 | Git exclude entry missing | File may appear in `git status` | `item status` / `repo status` | `repo repair` | Exclude block is rewritten |
| 4 | Wrong-target symlink | Repo path points at another store path | `item status` / `repo status` | `item repair --force <PATH>` | Requires explicit force |
| 5 | Regular file at repo path | User-created file occupies expected symlink path | `item status` / `repo status` | Manual decision | Repair refuses to overwrite |
| 6 | Repository path moved | Local index root points at the old path | `repo status` / next operation | Normal operation or `repo repair` when `git_common_dir` still matches | Index metadata can be refreshed |
| 7 | Repository directory renamed | Local root changed but clone identity may still match | `repo status` / next operation | Normal operation or `repo repair` | `git_common_dir` match can reuse `RepoId` |
| 8 | `repos/<repo-store-dir>` renamed | `index.json` points to old locator | `store verify` / load failure | `store rebuild-index` | Manifest `repo_id` remains identity |
| 9 | `index.json` deleted | Local Git metadata cache is gone | Missing index or empty repo list | `store rebuild-index` then `repo reclaim` | Rebuild restores `repo_id` and `repo_store_dir` only |
| 10 | Re-clone on same machine | New clone has no cache match | `item add` / `repo status` may print reclaim hint before using a fresh association | `repo reclaim` then `repo repair` | No automatic reclaim from hints |
| 11 | PC migration | Only `repos/` is restored | Missing local cache | `store rebuild-index`, `repo reclaim`, `repo repair` | Main portability workflow |
| 12 | Corrupted manifest | JSON parse fails | `store rebuild-index` / `store verify` | Restore manifest from backup or manual repair | Scanner reports and skips corrupted manifest where safe |
| 13 | Duplicate `RepoId` | Two manifests claim same repository identity | Scanner duplicate check | Manual resolution | Rebuild/reclaim fail without mutation |
| 14 | Duplicate `ItemId` | Two items claim same item identity | Scanner duplicate check | Manual resolution | Rebuild/reclaim fail without mutation |
| 15 | Unassociated repo in index | `root: None` after rebuild | `repo list --verbose` | `repo reclaim` from the desired clone | Not a deletion signal |
| 16 | Local clone path missing | `root: Some(path)` no longer exists | `store verify` / `store gc` planning | `repo reclaim` from a current clone | GC still deletes only `orphaned` items |
| 17 | Orphaned item | Item has no valid claimant | Scanner or GC planning | `store gc` after confirmation | Only `orphaned` is GC-eligible |

---

## Recovery Workflows

### PC Migration

```text
1. Restore repos/
2. Run: shelfbox store rebuild-index
3. Enter the current Git repository
4. Run: shelfbox repo reclaim
5. Select the existing RepoId
6. Run: shelfbox repo repair
```

Expected result:

* Existing managed items are preserved.
* Current clone is associated with the selected `RepoId`.
* Symlinks and exclude entries are repaired.
* No ownership transfer occurs.

---

### Re-clone Recovery

```text
1. Clone repository again
2. Keep or restore the existing repos/
3. Run: shelfbox store rebuild-index
4. Enter the new clone
5. Run: shelfbox repo reclaim
6. Select the matching repository
7. Run: shelfbox repo repair
```

Expected result:

* The new clone uses the existing `RepoId` only after explicit selection.
* Existing items are not moved or merged.
* Repair restores local working tree integration.

---

## Recoverability Summary

| Broken component | Recoverable? | Needed action |
|---|---|---|
| Symlink missing | Yes | `item repair` or `repo repair` |
| Symlink wrong target | Yes, with explicit force | `item repair --force` |
| Regular file at repo path | Manual | User decides |
| Exclude entry missing | Yes | `repo repair` |
| `index.json` missing | Yes | `store rebuild-index`, then `repo reclaim` |
| Repository store directory renamed | Yes | `store rebuild-index` |
| New clone or new PC | Yes | `repo reclaim`, then `repo repair` |
| Store item missing | No | Restore data from backup |
| Corrupted manifest | Manual | Restore or repair manifest |
| Duplicate identities | Manual | Resolve duplicate manifests/items |

---

## Design Invariants

| Invariant | Justification |
|---|---|
| `repos/` is canonical | Cache files can be rebuilt from manifests |
| `RepoId` is the only repository identity | Prevents accidental merge based on path, name, or remote |
| `manifest.json` writes are atomic | A crash mid-write leaves the previous manifest intact |
| `index.json` writes are atomic | Same guarantee for the local cache |
| `store_path` is repo-store-relative | Manifests are portable across store locations and machines |
| `identity_hints` are not proof | Candidate ranking and hints do not imply identity |
| Reclaim is explicit | Association changes require user intent |
| Repair is ownership-neutral | Local integration can be fixed without changing ownership |
| GC is conservative | Only confirmed `orphaned` items may be deleted, and manifests are saved before file removal |

---

## Recovery Test Scenarios

The recovery integration tests are compiled inside `shelfbox-core` so they can
exercise crate-private implementation modules without widening the public API.
The recovery scenarios live in
`crates/shelfbox-core/src/integration_tests/recovery_integration.rs`.

| Scenario | Test | Key assertion |
|---|---|---|
| Move repository path | `move_repository_path_reuses_repoid_via_git_common_dir` | Existing `RepoId` is reused when local Git metadata matches |
| Rename repository directory | `rename_repository_directory_reuses_repoid_via_git_common_dir` | Existing `RepoId` is reused when local Git metadata matches |
| Rename `repos/<repo-store-dir>` | `renamed_repo_store_dir_rebuild_index_restores_locator_and_repair_succeeds` | `store rebuild-index` restores the locator and repair succeeds after reclaim |
| Delete `index.json` and rebuild | `delete_index_and_rebuild_restores_repoid_and_store_dir_without_git_metadata` | Rebuilt index contains `repo_id` and `repo_store_dir`, but no Git metadata |
| Re-clone and reclaim | `reclone_reclaim_associates_existing_repoid_after_fresh_repoid` | New clone uses old `RepoId` only after explicit reclaim |
| Repair after reclaim | `repair_after_reclaim_restores_symlinks_and_exclude_entries` | Symlinks and exclude entries are restored |
| Reject reclaim with current items | `reclaim_rejects_current_repo_with_items_before_mutation` | Reclaim precondition errors before mutation |
| Duplicate `repo_id` | `duplicate_repoid_makes_rebuild_index_and_reclaim_fail_hard` | `store rebuild-index` and reclaim fail hard |
| Duplicate `item_id` | `duplicate_itemid_makes_rebuild_index_and_reclaim_fail_hard` | `store rebuild-index` and reclaim fail hard |
| GC safety | `gc_does_not_delete_unreachable_repos_or_items` | Non-`orphaned` items and repository stores survive GC |
