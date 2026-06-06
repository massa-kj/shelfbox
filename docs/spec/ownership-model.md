# Ownership State Machine Specification

# 1. Purpose

This document defines the formal ownership model for `shelfbox`.

The ownership model exists to answer the following questions deterministically:

* Who owns a shelved item?
* Which repository is allowed to repair or reclaim it?
* When is an item reclaimable?
* When is garbage collection safe?
* Which commands are allowed to mutate ownership state?

This specification is the authoritative source for ownership semantics across:

* `repair`
* `restore`
* `repo repair`
* `repo gc`
* `repo adopt`
* directory namespace shelving
* future provenance and snapshot systems

---

# 2. Design goals

The ownership model must satisfy the following goals.

## 2.1 Deterministic recovery

Recovery decisions must never depend on heuristics, timestamps, or content similarity.

Ownership transitions must be derivable from explicit metadata only.

---

## 2.2 Separation of concerns

The ownership model must remain independent from:

* filesystem layout
* symlink implementation
* content hashing
* future deduplication
* future snapshot/versioning

Ownership identity is not content identity.

---

## 2.3 Explicit ownership transfer

Ownership transfer must never occur implicitly.

Operations that change ownership must always:

* be explicit
* be auditable
* require user intent

---

## 2.4 Local-only repair

`repair` operations must never mutate ownership.

Repair is strictly an integrity operation.

---

## 2.5 Salvage-first policy

Loss of metadata must never immediately imply loss of ownership.

Objects remain reclaimable until explicitly garbage-collected.

---

# 3. Terminology

## 3.1 Repository Identity (`RepoId`)

Stable logical identity assigned to a repository registration.

Properties:

* globally unique ULID
* stable across repairs
* independent from filesystem path
* independent from `git_common_dir`

A repository move or reclone may create a new `RepoId`.

---

## 3.2 Item Identity (`ItemId`)

Stable logical identity assigned when an item is first shelved.

Properties:

* immutable
* globally unique ULID
* identifies ownership lineage
* independent from content
* independent from filesystem path

`ItemId` MUST NOT encode content identity.

---

## 3.3 Ownership Binding

Relationship between:

```text
(ItemId) → (RepoId)
```

A binding represents the current logical owner of an item.

---

## 3.4 Attachment

An item is considered attached when:

* a valid ownership binding exists
* the owner repository is considered active
* the item is reachable from the active manifest

Attachment does not require the symlink to currently exist.

---

## 3.5 Reclaimability

An item is reclaimable when ownership continuity can still be established deterministically.

Reclaimability is distinct from current attachment state.

---

## 3.6 Orphan

An orphan is an item for which no valid ownership claimant exists.

Orphan status is terminal unless manually overridden.

---

## 3.7 Orphan classification mapping

The failure matrix defines five orphan classes.  Their mapping to ownership
states is as follows:

| Orphan class | Ownership state | Notes |
|---|---|---|
| `unowned` | `orphaned` | No manifest entry; no claimant found |
| `unreachable` | `unreachable` | Manifest entry exists; repo path gone |
| `detached` | `detached` | Intentionally unlinked; store retained |
| `stale` | `stale` | Superseded by newer repo identity |
| `abandoned` | `unreachable` or `stale` | Worktree deleted; classification depends on whether the worktree held a distinct repo identity |

Orphan classes are diagnostic labels used by `repo gc` and `status` output.
Ownership states are the authoritative model.

---

# 4. Ownership object model

## 4.1 Repository identity

```text
RepoIdentity {
    repo_id
    git_common_dir
    created_at
}
```

`git_common_dir` is discovery metadata only.

It MUST NOT be treated as ownership identity.

---

## 4.2 Item identity

```text
ItemIdentity {
    item_id
    origin_repo_id
    created_at
}
```

`origin_repo_id` records the repository that originally created the item.

It is immutable.

---

## 4.3 Ownership binding

```text
OwnershipBinding {
    item_id
    current_owner_repo_id
    state             -- one of the states defined in section 5
    updated_at
}
```

The ownership state machine operates on this object.

When ownership is transferred via `repo adopt`, the old binding transitions to
`adopted` and a new binding is created in `attached` state for the new owner.
Both bindings are retained for auditability.

---

# 5. Ownership states

## 5.1 Attached

```text
attached
```

Definition:

* item has an active owner
* item is reachable from owner manifest
* ownership is valid

Properties:

* reclaimable
* repairable
* restorable

---

## 5.2 Detached

```text
detached
```

Definition:

* item intentionally removed from active manifest
* store item intentionally retained
* ownership preserved

Typical source:

```text
item restore --keep-store
```

Properties:

* reclaimable
* not garbage-collectable automatically

---

## 5.3 Unreachable

```text
unreachable
```

Definition:

* owner repository can no longer be resolved
* no replacement owner established
* ownership continuity still exists

Examples:

* repository deleted
* repository path disappeared
* index loss

Properties:

* reclaimable
* protected from automatic GC

---

## 5.4 Stale

```text
stale
```

Definition:

* ownership superseded by a newer repository identity
* old ownership still exists
* reclamation remains possible

Examples:

* reclone
* repository move
* worktree promotion

Properties:

* reclaimable
* protected from automatic GC

---

## 5.5 Adopted

```text
adopted
```

Definition:

* ownership explicitly transferred to a different repository identity
* old binding superseded by a `repo adopt` operation
* a new binding is created in `attached` state under the new owner

Typical source:

```text
repo adopt
```

Properties:

* NOT reclaimable by the original owner (already reclaimed by new owner)
* NOT garbage-collectable automatically
* retained as an auditable record of ownership transfer

---

## 5.6 Orphaned

```text
orphaned
```

Definition:

* no deterministic ownership claimant exists
* reclaimability lost

Properties:

* eligible for GC after confirmation

---

# 6. State transitions

## 6.1 Allowed transitions

```text
attached
    -> detached       (item restore --keep-store)
    -> unreachable    (repo deleted / path disappeared)
    -> stale          (reclone / repo moved / index lost)

detached
    -> attached       (re-link)
    -> orphaned       (gc after confirmation)

unreachable
    -> attached       (repo adopt: ownership reclaim by same identity)
    -> adopted        (repo adopt: ownership transfer to new identity)
    -> orphaned       (gc after confirmation)

stale
    -> adopted        (repo adopt: old binding superseded)
    -> orphaned       (gc after confirmation)

adopted
    (terminal — no further transitions from old binding)
```

Note: when `stale -> adopted` occurs, `repo adopt` simultaneously creates a
new binding in `attached` state for the new owner.

Note: `unreachable -> attached` applies when the same logical repository
re-registers (e.g. reclone to same identity). `unreachable -> adopted` applies
when a different repository identity claims the items.

---

## 6.2 Forbidden transitions

The following transitions are forbidden by invariant.

```text
repair
    -> any ownership mutation

gc
    -> deletion without orphaned state

adopt
    -> implicit ownership transfer

restore
    -> ownership transfer
```

---

# 7. Command authority model

Ownership transitions are restricted by command capability.

---

## 7.1 `item repair`

Allowed operations:

* recreate symlink
* restore local integrity

Forbidden operations:

* ownership reassignment
* orphan resolution
* adoption
* state mutation

`repair` is local-only.

---

## 7.2 `repo repair`

Allowed operations:

* manifest reconstruction
* symlink reconstruction
* root metadata repair

Forbidden operations:

* ownership transfer
* orphan resolution
* implicit adoption

Manifest reconstruction MUST NOT invent ownership identity.

---

## 7.3 `item restore`

Allowed transitions:

```text
attached -> detached
```

when `--keep-store` is used.

Otherwise the ownership binding is removed entirely.

---

## 7.4 `repo adopt`

Allowed transitions (old binding):

```text
stale       -> adopted
unreachable -> adopted   (ownership transfer case)
unreachable -> attached  (ownership reclaim case: same identity)
```

For every `stale -> adopted` or `unreachable -> adopted` transition, `repo adopt`
MUST simultaneously create a new binding in `attached` state for the new owner.

Requirements:

* explicit user intent
* auditable operation
* deterministic ownership proof
* adopted state retained in old binding for audit trail

`repo adopt` is the only command allowed to transfer ownership.

---

## 7.5 `repo gc`

Allowed operations:

* delete orphaned items

Forbidden operations:

* reclaimability judgement by heuristic
* deletion of stale items
* deletion of detached items

---

# 8. Recovery rules

## 8.1 Repair boundary

Repair is bounded to integrity restoration only.

Repair MUST NOT:

* create ownership
* transfer ownership
* synthesize metadata
* reinterpret stale state

---

## 8.2 Manifest reconstruction

Manifest reconstruction is permitted only when:

* ownership continuity remains provable
* item/store path mapping is deterministic

Reconstruction MUST NOT generate new `ItemId`s.

---

## 8.3 Store item loss

Missing store data is terminal.

The system MUST report:

```text
CannotFix
```

No placeholder reconstruction is permitted.

---

# 9. Garbage collection policy

GC policy is ownership-aware.

---

## 9.1 Automatically protected states

The following states MUST NOT be auto-collected:

* attached
* detached
* stale
* unreachable
* adopted

---

## 9.2 GC-eligible state

Only:

```text
orphaned
```

is eligible for deletion.

Deletion still requires explicit confirmation.

---

# 10. Identity invariants

The following invariants are mandatory.

---

## 10.1 Ownership identity is not content identity

```text
ItemId != content hash
```

Future deduplication or snapshots MUST use separate identifiers.

---

## 10.2 Ownership transfer is explicit-only

No operation may silently transfer ownership.

---

## 10.3 Repair is ownership-neutral

Repair operations must never mutate ownership state.

---

## 10.4 Manifest reconstruction is deterministic-only

No recovery path may invent ownership metadata heuristically.

---

## 10.5 Reclaimability precedes GC

Potentially reclaimable items must remain protected.

---

# 11. Future extensibility

This model is intentionally designed to support future extensions without redefining ownership semantics.

Examples:

* snapshots
* historical lineage
* deduplicated blobs
* multiple stores
* provenance metadata
* remote synchronization
* namespace-based directory shelving

These systems may extend metadata, but MUST NOT redefine ownership semantics established in this document.

---

# 12. Non-goals

The ownership model does not attempt to define:

* content versioning
* synchronization semantics
* distributed conflict resolution
* cryptographic integrity
* deduplication policy
* storage optimization

These are separate layers.

---

# 13. Relationship to failure matrix

The ownership model exists to formalize the recovery semantics already implied by the failure matrix.

The failure matrix remains the operational reference.

This document defines the ownership invariants that constrain future implementations.
