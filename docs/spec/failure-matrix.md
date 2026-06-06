# Failure Matrix

This document catalogues every known failure mode in `shelfbox`, describing
how each one manifests, how it is detected, and how to recover from it.

The guiding hierarchy is:

```
Don't break
  ↓
If broken, detect it
  ↓
If detected, recover from it
  ↓
Recovery must be deterministic
```

---

## Failure modes

| # | Scenario | How it breaks | Detection | Recovery command | Notes |
|---|----------|--------------|-----------|-----------------|-------|
| 1 | **Symlink deleted** (e.g. `rm .env`) | Repo-side symlink gone; store file intact | `item status` / `repo status` → `link_exists: false` | `item repair .env` | Safe to repair; store file unaffected |
| 2 | **Store item deleted** (data loss) | Store file gone; symlink dangling | `item status` / `repo status` → `store_exists: false` | Manual recovery (data gone) | `repair` returns `CannotFix`; data_loss_warnings emitted |
| 3 | **Manifest lost** (`manifest.json` deleted) | No items listed; store files still exist | `repo status` → 0 items, orphan store items | `repo repair --yes` | Deterministic rebuild from `items/` layout |
| 4 | **Index lost** (`index.json` deleted) | New ULID generated; old store dir becomes unreachable | Fresh `repo status` → 0 items, but symlinks still in repo | `repo repair --yes` in each repo | Old repo store dir is orphaned under `repos/`; symlinks still point to old store |
| 5 | **Repo moved** (entire `mv ~/src/api ~/work/api`) | Both `root` and `git_common_dir` change; new ULID created; old shelf inaccessible | Fresh `repo status` → 0 items; next `repo repair` in any repo with a matching `git_common_dir` marks old items `stale` automatically | `repo repair` applies `Attached → Stale`; then `repo adopt --from <old-id>` to reclaim | `detect_transitions::run()` handles the transition automatically on next `repo repair` |
| 6 | **Repo accessed via linked worktree** | Different `root` but same `git_common_dir`; two-stage lookup resolves to same ULID | Transparent (no warning) | None needed | Automatic via `git_common_dir` lookup |
| 7 | **Interrupted write — manifest** | `manifest.json.tmp` written but `rename(2)` not called | Startup: `manifest.json` unchanged (temp file is a sibling) | None needed | Atomic `rename(2)` guarantees `manifest.json` is never half-written |
| 8 | **Interrupted write — move phase** | File moved to store but manifest not yet saved | `repo status` → orphan store item (no manifest entry) | `repo repair --yes` | Rebuild candidate if repo-side symlink exists and target matches |
| 9 | **Concurrent `add` from two processes** | Advisory `flock` on `<store>/.lock`; second process blocked | `AppError::StoreLocked` returned to second process | Retry after first completes | Shared-lock mode allows concurrent reads; exclusive-lock for writes |
| 10 | **Store partially copied** (e.g. interrupted `cp -r`) | Some `items/` files missing, manifest intact | `repo status` → some items `store_exists: false` | Manual recovery (copy missing files from source) | `repair` cannot recreate missing data |
| 11 | **Wrong-target symlink** (e.g. leftover from reclone) | Symlink at repo path points to a different store | `item status` → `link_valid: false` | `item repair --force .env` | Without `--force`, `repair` refuses to overwrite |
| 12 | **Regular file at repo path** (user placed file) | File exists at path where symlink should be | `item status` → `link_exists: true`, `link_valid: false` | Investigate; `item restore` if desired | `repair` returns `PathIsRegularFile`; refuses to overwrite |
| 13 | **Exclude entry missing** | File visible to `git status` (but only cosmetically) | `repo status` → `in_exclude: false` | `repo repair` | Safe; exclude block is rewritten atomically |
| 14 | **Store relocated** (store moved to new path) | All absolute paths in index wrong; `config.store` must be updated | Every command fails with path errors | Update `config.toml` or `$SHELFBOX_STORE` then `repo repair` | `store_path` in manifest is repo-store-relative; only `index.json` roots need fixing |
| 15 | **Premature `store gc` on reclaimable items** | Repo root gone; items in `Stale`, `Unreachable`, or `Detached` state; user attempts `store gc --yes` | `store gc` loads each repo's manifest and counts reclaimable items before deletion | Automatic — deletion is skipped even with `--yes`; no recovery needed | `Orphaned` and `Adopted` items are the only ones `store gc` will delete |

---

## Recoverability summary

| Broken component | Recoverable? | What's needed |
|-----------------|-------------|---------------|
| Symlink missing | Yes | `item repair` |
| Symlink wrong target | Yes (with `--force`) | `item repair --force` |
| Regular file at repo path | Manual | User decides |
| Exclude entry missing | Yes | `repo repair` |
| Manifest missing | Yes | `repo repair --yes` |
| Index missing | Partial | Re-register each repo; old store dirs become orphans |
| Store item missing | **No** | Data lost; manual recovery only |
| Store partially copied | **No** (for missing items) | Copy missing files from source |
| Repo moved (same git_common_dir) | Yes, automatic | `context::build` updates root via two-stage lookup |
| Repo moved (new clone, different git_common_dir) | Partial | `shelfbox repo adopt --from <old-repo-id>` |
| Items in `Stale` or `Unreachable` state | Yes | `repo adopt --from <old-id>` (run `repo repair` first to detect) |
| Items in `Detached` state | Yes | `item relink <PATH>` |
| Store dir with reclaimable items (gc attempted) | Prevented | `store gc` always skips repos with reclaimable items |

---

## Design invariants

These invariants underpin the recoverability guarantees above.

| Invariant | Justification |
|-----------|--------------|
| `manifest.json` writes are atomic (`rename(2)`) | A crash mid-write leaves the previous manifest intact; no partial JSON |
| `index.json` writes are atomic | Same guarantee for the global index |
| `items/<repo-relative-path>` layout is deterministic | Manifest can always be reconstructed from the store tree without guessing |
| `repair` never deletes store items | Follows "salvage-first" policy; orphans require explicit `--yes` to remove |
| `repair` requires `--force` for wrong-target symlinks | Prevents silently masking stale links from reclones or copied repos |
| `repair` refuses to overwrite regular files | Prevents data loss when the user has placed their own file at the path |
| Advisory `flock` on all store access | Prevents manifest/index inconsistency from concurrent `shelfbox` processes |
| `store_path` is repo-store-relative | `manifest.json` is portable across store relocations on the same machine |

---

## Test coverage

| Scenario | Covered by |
|----------|-----------|
| Symlink deleted, repair recreates | `repair_recreates_missing_symlink` |
| Store item deleted (data loss) | `repair_returns_store_missing_when_store_item_gone`, `doctor_fix_records_cannot_fix_for_store_missing` |
| Manifest lost, rebuild from store | `doctor_fix_rebuilds_manifest_when_missing` |
| Index lost, fresh context created | `chaos::index_deleted_creates_fresh_context_with_empty_manifest` |
| Repo moved via linked worktree | `chaos::worktree_add_reuses_repo_ulid`, `chaos::worktree_shelved_items_visible_from_linked_worktree` |
| Interrupted write (manifest) | Atomic rename; no test needed (OS guarantee) |
| Interrupted write (move phase) | `doctor_fix_rebuilds_manifest_when_missing` (simulates via orphan injection) |
| Concurrent reads | `chaos::concurrent_read_locks_are_shared` |
| Partial store corruption | `chaos::partial_store_corruption_shows_mixed_status` |
| Wrong-target symlink | `repair_rejects_wrong_target_symlink_without_force`, `repair_force_relinks_wrong_target_symlink` |
| Regular file at repo path | `repair_refuses_to_overwrite_regular_file` |
| Exclude entry missing, repair restores | `doctor_fix_repairs_missing_exclude_entry` |
