# Data Model

This document describes the persistent data structures used by shelfbox.

For architectural concepts, see `architecture-overview.md`.

---

# Store Structure

A shelfbox store contains all persistent state.

Typical layout:

```text
<store>/
├── meta.json
├── index.json
└── repos/
    └── <repo-name>-<repo-id>/
        ├── manifest.json
        └── items/
```

On Unix, the store root and each repository store directory are created with
mode `0700` (owner-only access).

---

# meta.json

Store-level metadata.

Purpose:

* Store identity
* Provenance information

Contains:

```text
store_id
created_at
hostname
```

Properties:

* Created once
* Stable for the lifetime of the store
* Not used for ownership decisions

---

# index.json

Global repository registry.

Purpose:

```text
Repository discovery
```

Contains one entry for every known repository.

Each entry records:

```text
repo_id
root
git_dir
git_common_dir
store_dir
last_seen_at
```

Properties:

* One file per store
* Repository identities are globally unique
* Repository moves may create new identities

---

# Repository Store

Each repository receives its own store directory.

```text
repos/
└── my-project-01ABC.../
```

Purpose:

* Manifest storage
* Shelved item storage

The repository directory remains even if the repository is later deleted.

---

# manifest.json

Repository-level metadata.

Purpose:

```text
Track managed items.
```

Contains:

```text
repo metadata
managed items
namespace entries
```

The manifest is the primary repository state file.

---

# Item

A managed file or directory entry.

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
* Preserved across adoption
* Used for provenance

---

## path

Repository-relative path.

Example:

```text
.env
```

or

```text
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

---

## ownership_state

Current ownership state.

Examples:

```text
attached
detached
stale
unreachable
adopted
orphaned
```

Formal definitions are provided in:

```text
spec/ownership-model.md
```

---

# Namespace

A namespace represents a managed directory grouping.

Example:

```text
secrets/
```

Namespace entries contain:

```text
path
created_at
updated_at
```

Membership is derived from item paths.

No member list is stored.

---

# Repository Identity

A repository identity represents a logical repository owner.

Properties:

```text
repo_id
git_common_dir
```

Repository identity is independent of filesystem location.

A repository move or reclone may create a new identity.

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

| Data                 | Source of Truth           |
| -------------------- | ------------------------- |
| Store identity       | `meta.json`               |
| Repository registry  | `index.json`              |
| Repository state     | `manifest.json`           |
| Actual file contents | `items/`                  |
| Ownership semantics  | `spec/ownership-model.md` |

---

# Related Documents

* `architecture-overview.md`
* `design-decisions.md`
* `spec/ownership-model.md`
