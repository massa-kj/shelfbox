# Data Model

This document describes the persistent data structures used by shelfbox.

For architectural concepts, see `architecture-overview.md`.

---

# Store Structure

A shelfbox store contains all persistent state that must survive repository
moves, reclones, and PC migration.

Typical layout:

```text
<store>/
├── meta.json
├── index.json
└── repos/
    └── my-project/
        ├── manifest.json
        └── items/
```

Repository store directory names are human-readable locators only. They are not
identity. If a name is already used, shelfbox appends a numeric suffix:

```text
repos/
├── api/
├── api-2/
└── api-3/
```

Users may rename directories under `repos/`; the `RepoId` in `manifest.json`
remains the repository identity.

On Unix, the store root and each repository store directory are created with
mode `0700` (owner-only access).

---

# meta.json

Store-level local metadata.

Purpose:

```text
Store identity and provenance display.
```

Contains:

```text
store_id
created_at
hostname
```

Properties:

* Created once
* Stable for the lifetime of the store
* Not used for repository or item ownership decisions

---

# index.json

Local cache that maps `RepoId` values to the current or last-seen Git metadata
on this machine.

`index.json` is not canonical. It must be safe to delete and rebuild from
`repos/*/manifest.json`.

Example:

```json
{
  "repos": {
    "01ABC...": {
      "repo_store_dir": "my-project",
      "root": "/work/my-project",
      "git_dir": "/work/my-project/.git",
      "git_common_dir": "/work/my-project/.git",
      "last_seen_at": "2026-06-11T00:00:00Z"
    }
  }
}
```

Fields:

| Field | Meaning |
|---|---|
| `repo_store_dir` | Directory under `<store>/repos/` containing the manifest |
| `root` | Optional current or last-seen Git worktree root |
| `git_dir` | Optional Git directory cache |
| `git_common_dir` | Optional common Git directory cache for linked worktrees |
| `last_seen_at` | Diagnostic and candidate display metadata |

When rebuilt from manifests alone, `root`, `git_dir`, and `git_common_dir` may
be missing. Commands must treat missing Git metadata as an unassociated cache
state, not as data loss.

---

# Repository Store

Each logical repository has one repository store directory.

```text
repos/
└── my-project/
    ├── manifest.json
    └── items/
```

Purpose:

* Canonical repository manifest storage
* Shelved item storage

The repository store directory remains even if no current Git clone is
associated with its `RepoId`.

---

# manifest.json

Repository-level canonical metadata.

Example:

```json
{
  "version": 3,
  "repo_id": "01ABC...",
  "created_at": "2026-06-10T00:00:00Z",
  "identity_hints": {
    "remote_hints": ["github.com/org/project"],
    "repo_name_hints": ["project"],
    "last_attached_at": "2026-06-10T00:00:00Z"
  },
  "items": [
    {
      "item_id": "01DEF...",
      "origin_repo_id": "01ABC...",
      "path": ".env",
      "store_path": "items/.env",
      "ownership_state": "attached",
      "created_at": "2026-05-21T11:52:42Z",
      "updated_at": "2026-05-21T11:52:42Z"
    }
  ]
}
```

`manifest.json` must not store:

```text
root
git_dir
git_common_dir
absolute paths
hostname
namespace entries
link implementation metadata
```

These values are either local cache data, runtime behavior, or UI presentation.

---

# identity_hints

Hints are used only to rank and display reclaim candidates. They are never
proof of identity and must never trigger automatic reclaim.

Fields:

| Field | Meaning |
|---|---|
| `remote_hints` | Normalized remote hints such as `github.com/org/project` |
| `repo_name_hints` | Recent repository directory names, most recent first |
| `last_attached_at` | Last successful explicit association or repair timestamp |

Rules:

* `remote_hints` are deduplicated.
* `repo_name_hints` are deduplicated, most recent first, and capped at 5.
* Absolute paths are not recorded in `identity_hints`.

---

# Item

A managed file entry.

Items are path-addressed files. Directory operations are command behavior over
matching item paths; they do not add a persisted item kind or namespace record.

Important fields:

```text
item_id
origin_repo_id
path
store_path
ownership_state
created_at
updated_at
```

---

## item_id

Unique identity assigned when the item is first shelved.

Properties:

* Immutable
* Globally unique
* Independent of content

---

## origin_repo_id

Repository identity that originally created the item.

Properties:

* Immutable
* Used for provenance
* Not rewritten by reclaim or repair

---

## path

Repository-relative path.

Example:

```text
.env
secrets/api_key.txt
```

---

## store_path

Path relative to the repository store.

Example:

```text
items/.env
```

Properties:

* Portable
* Independent of store root location
* Deterministic from the repo-relative path

---

## ownership_state

Current ownership state.

Supported states:

```text
attached
detached
unreachable
orphaned
```

Formal definitions are provided in `docs/spec/ownership-model.md`.

---

# Repository Identity

A repository identity represents a logical repository owner.

Properties:

```text
repo_id
```

`RepoId` is the only repository identity. It is independent of:

* Filesystem location
* Git clone path
* Git remote URL
* Repository store directory name

---

# Item Identity

An item identity represents ownership lineage.

Properties:

```text
item_id
origin_repo_id
```

Item identity is independent of:

* Content
* Path
* Symlink target

---

# Source of Truth

| Data | Source of Truth |
|---|---|
| Store local metadata | `meta.json` |
| Repository registry cache | `index.json` |
| Repository identity and item metadata | `repos/<repo-store-dir>/manifest.json` |
| Actual file contents | `repos/<repo-store-dir>/items/` |
| Ownership semantics | `docs/spec/ownership-model.md` |

The `repos/` directory is the canonical persistent store. Preserving `repos/`
must be enough to rebuild local cache files and recover managed items.

---

# Related Documents

* `architecture-overview.md`
* `design-decisions.md`
* `docs/spec/ownership-model.md`
