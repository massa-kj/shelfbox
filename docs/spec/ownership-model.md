# Ownership State Machine Specification

Copy materialization does not change ownership semantics. The normative
copy-mode contract is defined in [`copy-mode.md`](./copy-mode.md); this document
remains authoritative for ownership state and transitions.

# 1. Purpose

This document defines the formal ownership model for `shelfbox`.

The ownership model answers:

* Who owns a shelved item?
* Which repository is allowed to repair it?
* Which data is reclaimable?
* When is garbage collection safe?
* Which commands may mutate ownership state?

This specification is the authoritative source for ownership semantics across:

* `item add`
* `item restore`
* `item relink`
* `item repair`
* `repo reclaim`
* `repo repair`
* `store gc`

---

# 2. Design Goals

## 2.1 Stable Repository Identity

`RepoId` is the only repository identity.

It must remain stable across:

* Repository path moves
* Repository directory renames
* `repos/` directory renames
* PC migration
* Explicit reclaim
* Repair

Repository store directory names, Git remotes, and Git paths are not identity.

---

## 2.2 Explicit Association

A new Git clone is not automatically the same as an existing `RepoId`.

Association between the current clone and an existing `RepoId` may only happen
through explicit `repo reclaim`.

Candidate ranking may use hints, but ranking must never cause automatic
selection.

---

## 2.3 Repair Is Ownership-Neutral

Repair may restore local integration:

* Symlinks
* Git exclude entries
* Local index metadata
* Identity hints

Repair must not:

* Assign a different `RepoId`
* Transfer ownership
* Merge repositories
* Delete items
* Change item ownership state

---

## 2.4 Salvage-First Policy

Stored data should remain protected unless it is explicitly classified as safe
to delete.

Garbage collection may delete only `orphaned` items, and only after explicit
confirmation.

---

# 3. Terminology

## 3.1 Repository Identity (`RepoId`)

Stable logical identity assigned by shelfbox.

Properties:

* Globally unique
* Stored in `manifest.json`
* Independent from filesystem path
* Independent from Git remote URL
* Independent from repository store directory name

---

## 3.2 Item Identity (`ItemId`)

Stable logical identity assigned when an item is first shelved.

Properties:

* Immutable
* Globally unique
* Identifies ownership lineage
* Independent from content
* Independent from filesystem path

`ItemId` must not encode content identity.

---

## 3.3 Ownership Binding

Relationship between:

```text
ItemId -> RepoId
```

An item belongs to the repository whose manifest contains the item entry.

---

## 3.4 Reclaim

`repo reclaim` associates the current Git clone with an existing `RepoId`.

Reclaim:

* Updates `index.json`
* Updates `identity_hints`
* Does not move item data
* Does not copy item data
* Does not change item ownership state
* Does not repair symlinks or exclude entries

After reclaim, run `repo repair` to restore local working tree integration.

---

# 4. Ownership States

Supported states:

```text
attached
detached
unreachable
orphaned
```

---

## 4.1 Attached

```text
attached
```

Definition:

* Item belongs to the associated repository.
* Item is present in the repository manifest.
* The item is eligible for symlink and exclude repair.

Properties:

* Repairable
* Restorable
* Protected from GC

---

## 4.2 Detached

```text
detached
```

Definition:

* Item was intentionally detached while preserving the store copy.
* Ownership is preserved in the manifest.

Typical source:

```text
item restore --keep-store
```

Properties:

* Re-linkable with `item relink`
* Protected from GC

---

## 4.3 Unreachable

```text
unreachable
```

Definition:

* Manifest exists, but no current Git clone is associated with its `RepoId`.
* Ownership continuity still exists.

Examples:

* Store restored on another PC
* `index.json` rebuilt from `repos/`
* A previously associated clone no longer exists locally

Properties:

* Reclaimable by explicit `repo reclaim`
* Protected from GC

---

## 4.4 Orphaned

```text
orphaned
```

Definition:

* No valid ownership claimant exists.
* The item is eligible for explicit conservative GC.

Properties:

* Not repaired automatically
* May be deleted only by confirmed GC

---

# 5. State Transitions

Allowed transitions:

| From | To | Command | Meaning |
|---|---|---|---|
| none | `attached` | `item add` | New item is shelved |
| `attached` | removed | `item restore` | Store item is restored to the repo and manifest entry removed |
| `attached` | `detached` | `item restore --keep-store` | Store copy is preserved and ownership remains recorded |
| `detached` | `attached` | `item relink` | Detached item is re-linked |
| any valid state | unchanged | `repo reclaim` | Current clone is associated with an existing `RepoId` |
| any valid state | unchanged | `repo repair` / `item repair` | Local integration is repaired |
| `orphaned` | removed | `store gc` | Confirmed conservative deletion |

Automatic ownership transfer is not supported.

---

# 6. Garbage Collection Rules

GC may delete only items whose `ownership_state` is `orphaned`.

GC must not delete:

```text
attached
detached
unreachable
```

GC must not delete an entire repository store directory just because no current
Git clone is associated with it.

`root: None` in `index.json` means the entry was rebuilt from manifests or is
currently unassociated. It is not a deletion signal.

---

# 7. Namespace Policy

Namespace is UI presentation only.

Namespace must not be persisted as ownership or identity metadata. Directory
grouping can be derived from `item.path` when a UI wants to display items by
directory.

Namespace must not affect:

* Ownership
* Reclaim
* Repair
* Garbage collection

---

# 8. Safety Invariants

* `RepoId` is the only repository identity.
* `manifest.json` is canonical repository metadata.
* `index.json` is rebuildable local cache.
* `identity_hints` are not proof of identity.
* Reclaim requires explicit user selection.
* Repair never changes ownership state.
* GC deletes only confirmed `orphaned` items.
