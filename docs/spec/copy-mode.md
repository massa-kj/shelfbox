# v0.9.0 Copy Mode

## Purpose

Add copy materialization, which places a regular file on the repo side, so that shelfbox can be used in restricted environments where symlinks cannot be created.

Copy is an opt-in fallback, not the primary strategy. It does not change the default behavior for existing users, canonical data, or the ownership model.

## Non-goals for v0.9.0

v0.9.0 will not provide:

* automatic detection or synchronization of repo-side changes, or bidirectional synchronization
* bulk conversion of existing items in response to a config change
* copy history, conflict merging, or three-way merging
* persistence of a per-item strategy in the manifest
* materialization of leaf entries other than regular files, such as symlinks, devices, or sockets
* item- or repo-level strategy conversion commands
* repo-level bulk synchronization
* a generic transaction journal for every mutating command
* renaming or deprecating `restore --keep-store`, or migrating it to a dedicated detach command

Strategy conversion and repo-level bulk operations are deferred to v0.9.1.

## Invariants

* `repos/` and `manifest.json` are canonical, while `index.json` is a rebuildable cache.
* The meanings of `RepoId`, `ItemId`, and `ownership_state` do not change.
* The store-side file is always the canonical content.
* A repo-side copy is an editable materialized view; editing it alone does not make it canonical.
* Repo-side changes may be written back to the store only by an operation that explicitly specifies `--from repo`. The normal path is `item sync`; only retrieval of a detached item is handled by `item relink`.
* The copy/symlink distinction is not used to determine GC reachability or ownership.
* A repo-side materialization must not be tracked by Git and must have a Git exclude entry.
* A diverged copy is never implicitly overwritten, moved, or deleted.
* The exclude entry must be added and verified before a regular file is created on the repo side.
* A copy with a missing exclude entry is treated as an integrity error because it can leak content.

## Configuration

Add a default strategy for new materializations to the config.

```toml
materialization = "symlink" # default
# materialization = "copy"
```

Valid values are `symlink` and `copy`. An unknown value is a config error. When omitted, the value defaults to `symlink`, preserving existing behavior.

The setting is public through `shelfbox config get`, `list`, `explain`, and
`set`; `shelfbox config set materialization copy` is the supported activation
path.

The configured strategy is a default, not desired state, and is used only when:

* `item add` creates a new materialization
* repair or relink handles an item whose repo side is missing, so its previous strategy cannot be observed

Changing the config does not convert existing items. A mixture of symlink and copy items is a healthy state.

## State Model

### Separation of facts and policy

Do not compress filesystem, store, Git, and exclude state into one enum. First collect mutually independent facts, then let the policy layer evaluate them to determine status and whether an operation is allowed or rejected.

```rust
struct MaterializationFacts {
    repo_entry: RepoEntryKind,
    relation: MaterializationRelation,
    copy_content: CopyContentState,
    store_state: StoreState,
    git_state: GitState,
    exclude_state: ExcludeState,
}

enum RepoEntryKind {
    Missing,
    RegularFile,
    Symlink,
    Unsupported,
    Unreadable,
}

enum MaterializationRelation {
    ManagedSymlink,
    IsolatedRegularCopy,
    UnsafeHardlink,
    UnexpectedSymlink,
    NotApplicable,
    InspectionFailed,
}

enum CopyContentState {
    NotCompared,
    Equal,
    Diverged,
    Unreadable,
    ComparisonFailed,
}

enum StoreState {
    Present,
    Missing,
    Unsupported,
    Unreadable,
}

enum GitState {
    Untracked,
    Tracked,
    QueryFailed,
}

enum ExcludeState {
    Present,
    Missing,
    QueryFailed,
}
```

Do not represent `StoreMissing` as content divergence. Inability to compare, inability to read, and a missing store are distinct issues.

### Observed materialization

`ObservedMaterialization` is a presentation classification for the CLI and API that is derived from the facts.

* `Missing`
* `ManagedSymlink`
* `RegularCopy`
* `UnsafeHardlink`
* `UnexpectedSymlink`
* `UnsupportedEntry`
* `Unreadable`

A regular file itself carries no shelfbox-specific marker. It is evaluated as a `RegularCopy` by combining the path recorded in the manifest, store presence, file identity, Git state, exclude state, and content comparison.

### Content equality

Copy contents are compared byte for byte with a bounded-memory streaming comparison. mtime, owner, ACLs, and xattrs are not used to determine equality.

### Strategy difference

A difference between the configured strategy and the observed materialization is not an integrity failure.

* Status severity remains `Healthy`.
* Verbose output may show an informational note.
* The difference does not affect the status exit code, `ok`, or issues.
* Do not add `--strict-config`.

Operations appropriate to the observed materialization remain available after a configuration change. For example, after changing the config back to `symlink`, `item sync` can still operate on a remaining `RegularCopy`.

## Internal Design

### Abstraction boundary

Do not add copy responsibilities to the existing `LinkStrategy`. Keep `LinkStrategy` as the low-level abstraction responsible for creating, removing, and inspecting symlinks.

Add common materialization types and operations above it.

```rust
enum MaterializationStrategy {
    Symlink,
    Copy,
}

trait Materializer {
    fn inspect(/* ... */) -> Result<MaterializationFacts>;
    fn create(/* strategy, store_path, repo_path ... */) -> Result<()>;
    fn remove(/* facts, repo_path ... */) -> Result<()>;
}
```

The actual signatures should follow the existing plan/execute separation. `DefaultMaterializer` internally uses `LinkStrategy` and a copy transfer helper.

Operations such as status, repair, and sync collect facts and obtain typed decisions from a shared policy evaluator. Do not duplicate symlink-specific boolean checks or error names in each operation.

### Secure transfer helper

Centralize copy, cross-device move, and atomic replacement in a shared helper that guarantees:

* the final component of the source and destination is not followed as a symlink or reparse point
* escape from canonicalized repo/store roots and traversal through untrusted intermediate symlinks, junctions, or reparse points are rejected
* file identity and link count are inspected for the source, canonical store file, and repo copy, and hardlinks are rejected
* an unpredictable temp file is created in the destination directory on the same filesystem as the destination
* the temp file has owner-only-equivalent permissions from the moment it is created and never becomes group/world-readable before the copy completes
* data is copied to the temp file, flushed, `fsync`ed, and assigned permissions before atomic rename/replacement; the parent directory is also `fsync`ed on platforms where that is required
* a cross-device move falls back to `secure temp copy -> fsync -> atomic replace -> source remove`
* source identity is revalidated after opening the source and before commit, detecting replacement, conversion to a symlink, or conversion to a hardlink during the read
* failure removes the temp file and preserves any existing destination

Keep platform differences in no-follow open, atomic replacement, file identity, and reparse-point inspection inside an adapter. Refuse writes back to the store when a target cannot be verified safely.

### Metadata policy

Synchronization applies to file content.

* Initial `item add`: preserve the source file's store-side metadata and copy store permissions to the repo copy.
* `sync --from store`: make the repo copy's content and permissions match the store.
* `sync --from repo`: preserve existing store permissions and replace only the content.
* mtime may change as a result of an operation and is not used to determine synchronization.
* Copying owner, ACLs, and xattrs is not guaranteed.
* Even on platforms that cannot reproduce permissions exactly, never widen permissions beyond their current state.

Document platform-specific limitations and report them as CLI warnings.

### Durable operation recovery

Perform compensating rollback on ordinary error returns. Process termination, power loss, and panic do not execute rollback, so introduce durable operation records for the following operations, which update multiple persistent resources non-idempotently:

* `item add`
* normal `item restore` except `--keep-store`
* `item move`
* `item relink --from store`
* `item relink --from repo`
* each item operation within directory add/restore

`restore --keep-store`, repair, `sync --from store|repo`, and normal directionless relink are outside the v0.9.0 durable-record scope because they are bounded by atomic writes/replacements and retryable state transitions. Add them to the scope if implementation introduces a phase that cannot be retried safely.

An operation record contains at least:

* operation ID, kind, and `RepoId`
* strategy and sync direction
* repo path and store path
* summaries of manifest, exclude, and facts before the operation
* whether the exclude existed before the operation or was added by the operation
* completed phase
* source/destination file identity or a safe fingerprint
* recovery temp/backup path, identity, and purpose
* preconditions for forward recovery and rollback

Atomically write the record into the store before the first mutation, then `fsync` the record file and parent directory. Update it the same way after each durable phase and delete it only after completion. After deletion, `fsync` the parent directory again. The store write lock serializes shelfbox recovery and mutation.

Detect unfinished records when the next mutating command starts.

* If paths and identities remain as recorded, perform forward recovery or rollback according to the operation-specific rules.
* If facts have changed, such as another process creating a new entry at the repo path, return a recovery report without automatically overwriting anything.
* Do not start a new mutation until recovery completes.
* Read-only status does not modify the record and may report the unfinished operation as an issue.

Use a small recovery layer that makes the plans and phases of the relevant operations durable, not a generic transaction engine shared by every command.

#### Common phase-recovery rules

Operation-record phase names are persistent contracts shared by the implementation and failpoint tests. Each phase has expected states for the repo entry, store entry, manifest membership, exclude membership, and recovery temp/backup.

* Recovery does not trust the recorded phase alone; it verifies that the actual filesystem and metadata match the expected state.
* Record updates are write-ahead. In addition to the recorded phase's state, recovery accepts the next phase's expected state when only the immediately following mutation completed before a crash. If identities match, advance the record to the next phase before continuing recovery.
* Perform the table-defined forward or rollback action only when identities/fingerprints match.
* `forward` advances the operation explicitly requested by the user to completion.
* `rollback` restores the state from before the operation and removes only excludes added by that operation.
* `conflict` changes none of repo, store, manifest, or exclude, and preserves the record and recovery temp/backup.
* Treat a state that matches neither the current nor next phase, an identity query failure, or simultaneous satisfaction of multiple expected states as a conflict.
* Recovery actions themselves update the phase and call `fsync`, so the same rules apply if recovery is interrupted again.

The `recovery action` in each table is the only automatic recovery direction allowed in that phase. Permit rollback only before the commit point and forward recovery only at or after it. Automatic recovery does not perform the opposite action even when it would be physically possible. `completed` is not retained as a persistent phase; it means final postconditions were verified and the record was deleted.

Commit points:

* add: `store_transferred`
* restore: `repo_regularized`
* move: `store_transferred`
* directional relink: `content_synchronized`

#### Item add recovery truth table

`source` is the identity/fingerprint of the repo file at the start. `materialized` is the symlink/copy created by the operation.

| recorded phase | expected repo | expected store | manifest | exclude | recovery action |
| --- | --- | --- | --- | --- | --- |
| `record_created` | `source` | missing | absent | before-state | rollback: delete only the record |
| `exclude_written` | `source` | missing | absent | present | rollback: remove the exclude added by the operation |
| `store_transferred` | missing | `source` | absent | present | forward: materialize the repo |
| `repo_materialized` | `materialized` | `source` | absent | present | forward: save the manifest |
| `manifest_saved` | `materialized` | `source` | present | present | forward: verify postconditions and complete |

Any entry that differs from the table's expected state causes a conflict. In particular, if the repo path has been recreated after `store_transferred`, do not delete or overwrite that entry.

#### Item restore recovery truth table

Before permanently deleting the store file, normal restore stages it into a recovery temp on the same filesystem. A symlink item is atomically replaced with a regular file created from the store; an equal copy item retains the existing regular file. `repo-regular` means a regular file with the same content fingerprint as the store at the beginning of the operation.

| recorded phase | expected repo | expected store | manifest | exclude | recovery action |
| --- | --- | --- | --- | --- | --- |
| `record_created` | original materialization | canonical present | present | before-state | rollback: delete only the record |
| `repo_regularized` | `repo-regular` | canonical present | present | before-state | forward: stage the store into the recovery temp |
| `store_staged` | `repo-regular` | canonical missing, temp present | present | before-state | forward: remove the manifest entry |
| `manifest_removed` | `repo-regular` | canonical missing, temp present | absent | before-state | forward: apply keep-ignore policy |
| `exclude_updated` | `repo-regular` | canonical missing, temp present | absent | final-state | forward: delete the temp and complete |

If repo content changes at or after `repo_regularized`, perform neither automatic rollback nor forward recovery. Report a conflict and preserve both the repo file and recovery temp. Do not delete the recovery temp, which is the final copy of canonical data, before manifest removal and the exclude update are durable.

#### Item move recovery truth table

`old-materialized` / `new-materialized` and `old-store` / `new-store` identify the recorded identities and paths.

| recorded phase | expected repo | expected store | manifest | exclude | recovery action |
| --- | --- | --- | --- | --- | --- |
| `record_created` | old=`old-materialized`, new=missing | old=`old-store`, new=missing | old path | before-state | rollback: delete only the record |
| `destination_excluded` | old=`old-materialized`, new=missing | old=`old-store`, new=missing | old path | old+new | rollback: remove the new exclude added by the operation |
| `store_transferred` | old=`old-materialized`, new=missing | old=missing, new=`new-store` | old path | old+new | forward: move the repo materialization to the new path |
| `repo_moved` | old=missing, new=`new-materialized` | old=missing, new=`new-store` | old path | old+new | forward: save the new path in the manifest |
| `manifest_saved` | old=missing, new=`new-materialized` | old=missing, new=`new-store` | new path | old+new | forward: remove the old exclude |
| `exclude_finalized` | old=missing, new=`new-materialized` | old=missing, new=`new-store` | new path | new only | forward: verify postconditions and complete |

Any unexpected entry at either the old or new path causes a conflict. If another process created an entry at the destination, do not overwrite it during either rollback or forward recovery.

#### Directional relink recovery truth table

Directional relink treats content synchronization and `detached -> attached` as one operation. Retain the overwritten side's preimage in a recovery backup until the ownership update is durable.

| recorded phase | expected content | manifest | exclude | recovery action |
| --- | --- | --- | --- | --- |
| `record_created` | before direction is applied, backup absent | detached | before-state | rollback: delete only the record |
| `exclude_written` | before direction is applied, backup absent | detached | present | rollback: remove the exclude added by the operation |
| `content_synchronized` | store/repo equal, overwritten preimage backup present | detached | present | forward: save ownership as attached |
| `ownership_attached` | store/repo equal, backup present | attached | present | forward: delete the backup and complete |

`--from repo` backs up the store preimage; `--from store` backs up the repo preimage. If either repo or store changes from its expected identity after `content_synchronized`, report a conflict and preserve the backup. Normal directionless relink is outside this table.

## States and Default Behavior

| observed state | status | repair | sync from store | sync from repo |
| --- | --- | --- | --- | --- |
| missing | error | recreate with configured strategy | reject and suggest repair | reject |
| managed symlink | healthy | no-op | no-op | reject |
| equal regular copy | healthy | no-op | no-op | no-op |
| diverged regular copy | error (`content_diverged`) | report, do not modify | explicitly overwrite repo | explicitly update store |
| unsafe hardlink | error | reject, do not modify | reject | reject |
| unexpected symlink | error | replace with managed symlink only with `--force` | reject | reject |
| unsupported/unreadable | error | reject, do not modify | reject | reject |
| store missing/unreadable | error | reject, do not modify | reject | reject |

Additional conditions:

* Tracked state or a Git query failure is an error regardless of state, and all writes are rejected.
* A symlink with a missing exclude remains a warning, preserving existing behavior.
* A regular copy with a missing exclude is an error because it risks content leakage even when untracked. Reject content/materialization writes except for `repo repair` or a relink phase that repairs the exclude itself.
* An exclude query failure is an error.
* A strategy difference is informational and does not change the severity in this table.

## Command Specifications

### `item add`

`item add` creates a symlink or copy according to the configured strategy.

1. Validate path, regular-file type, Git-untracked state, manifest state, store destination, hardlink state, and path safety.
2. Create a durable operation record.
3. Add the exclude entry and verify it with `has_entry`.
4. Move the source file into the store with the secure transfer helper.
5. Materialize the repo side with the configured strategy.
6. Atomically save the manifest.
7. Verify postconditions and delete the operation record.

On an ordinary error, remove the materialization created by the operation, return the store file to its original path, restore the manifest snapshot from before the operation, and remove only the exclude entry added by this operation. Rollback may act only when the recorded identity matches, and must not overwrite an entry created by the user or another process.

If rollback itself fails, report both the original and rollback errors and retain the operation record for recovery on the next run. Directory add uses the same phases and records for each item, preserving the existing partial-success policy.

### `item status` / `repo status`

Status evaluates facts through the policy evaluator and returns at least:

```rust
enum StatusSeverity {
    Healthy,
    Warning,
    Error,
}

struct ItemStatus {
    status_schema_version: u32,
    path: String,
    configured_strategy: MaterializationStrategy,
    observed_materialization: ObservedMaterialization,
    materialization_exists: bool,
    materialization_valid: bool,
    content_state: CopyContentState,
    store_exists: bool,
    in_exclude: bool,
    not_tracked: bool,
    severity: StatusSeverity,
    issues: Vec<StatusIssue>,
    notes: Vec<StatusNote>,
    ok: bool,
    link_exists: Option<bool>,
    link_valid: Option<bool>,
}
```

`issues` use stable typed codes and may carry remediation and target-path kind where necessary. Information that does not affect integrity, such as a strategy difference, is separated into `notes`. CLI text and JSON present results from the same evaluator.

`materialization_valid` is a structural field indicating whether the entry kind and its relationship to the store are safe. Content divergence and exclude/Git problems belong in `content_state`, `issues`, and `severity`; do not mix them into this field's meaning.

JSON status contract:

* Include `status_schema_version = 2` in every item.
* Preserve the current outer JSON array/repo-report shape.
* The generic contract is `materialization_exists`, `materialization_valid`, and `observed_materialization`.
* `ok` is equivalent to `severity == Healthy`.
* Existing `link_exists` and `link_valid` remain their previous boolean values for symlink items and are `null` for copy items.
* Do not give the legacy booleans generic materialization semantics.
* Preserve existing field values for symlink items and add new fields additively.

Do not add a separate v1 compatibility output. Consumers handling copy items use the schema-v2 generic fields.

Severity and CLI exit code:

* `Healthy` / exit `0`: store, materialization, exclude, and Git state are healthy.
* `Warning` / exit `1`: canonical data exists but a non-destructive repair is
  needed, such as a missing exclude for a managed symlink.
* `Error` / exit `2`: missing materialization, missing store, tracked state, missing exclude for a copy, unsafe hardlink, unexpected entry, path escape, unfinished-operation conflict, and similar states.

For multiple items, use the highest severity as the exit code. `item status` and `repo status` are read-only and perform no repair, reclaim, manifest mutation, or operation recovery.

### `item repair`

Preserve its existing responsibility and do not modify excludes.

* When the exclude is missing or its query fails, do not create a materialization; suggest `repo repair`.
* For a missing materialization, verify exclude, untracked state, and store, then recreate it with the configured strategy.
* Equal copies and valid symlinks are no-ops.
* Report a diverged copy and do not overwrite it. Its status severity remains
  the `content_diverged` error.
* As before, replace a wrong-target symlink with a managed symlink only when `--force` is specified.
* Even with `--force`, do not overwrite regular files, diverged copies, hardlinks, or unsupported entries.
* Do not modify ownership, manifest identity, or exclude state.

### `repo repair`

As an existing repository-integration repair operation, repair materializations only for attached items. Excludes protect attached items and detached items that retain a repo-side materialization.

1. Build the following desired exclude set and apply it to the managed block.
2. Evaluate attached items through materialization policy.
3. Recreate missing materializations with the configured strategy.
4. Replace wrong-target symlinks only with `--force`.
5. Treat equal copies and valid symlinks as no-ops, and report diverged copies
   without modifying them.
6. Update index metadata and identity hints according to existing behavior.

Desired exclude set:

* Attached item: always include it, whether or not a repo entry exists.
* Detached item with a repo entry: always include it.
* Detached item with no repo entry: preserve it when already present in the current managed block, but do not add it.
* Unreachable/orphaned item: do not add it.

Do not overwrite regular-file content with `--force`. Restore excludes before materializations. Do not create or repair materializations for detached items; maintain only their exclude protection. If either a repo entry or the current managed block cannot be inspected safely while building the desired set, return an error without rewriting the block.

### `item sync`

Add a new command.

```sh
shelfbox item sync <PATH> --from store [--dry-run]
shelfbox item sync <PATH> --from repo [--dry-run] --yes
```

Direction is required, and specifying both directions is rejected. Decisions are based on facts and observed materialization, not configured strategy.

#### `sync --from store`

* Treat the store as authoritative and atomically replace the existing repo copy.
* Operate only on `RegularCopy`.
* Equal content is a no-op. Diverged content may be overwritten because direction was explicit.
* Reject `Missing` and suggest `item repair`.
* `ManagedSymlink` is a no-op.
* Reject hardlinks, unexpected/unsupported entries, tracked state, missing excludes, missing store, and inspection failures.

#### `sync --from repo`

* Treat the repo copy as authoritative and atomically replace the store.
* Operate only on an `attached` manifest item's `RegularCopy`.
* Equal content is a no-op.
* Reject managed symlinks, missing entries, hardlinks, unexpected/unsupported entries, tracked state, missing excludes, missing store, and inspection failures.
* Revalidate repo/store containment and file identity immediately before commit.
* Require `--yes` for an actual write. `--dry-run` returns the plan and facts without requiring `--yes`.

### `item restore`

Restore removes managed state; it is not a path for propagating edits.

* Managed symlink: restore a regular file from the store as in existing behavior.
* Equal regular copy: retain the repo copy and remove store/manifest management.
* Diverged regular copy: reject by default and require `item sync --from repo` or `item sync --from store` first.
* Missing, unsafe, or unexpected entries and a missing store are rejected.
* Normal restore removes management from the manifest/store and removes the exclude unless `--keep-ignore` is specified.
* Do not suggest `restore + add` as a way to propagate edits.

Protect store-data deletion and manifest updates with a durable operation record. Where possible, rename the store file into a recovery temp on the same filesystem before updating the manifest, then delete the temp after commit.

#### `restore --keep-store`

`restore --keep-store` remains a legacy detach operation for compatibility. Unlike normal restore, it does not return the repo side to a regular file.

* Retain the manifest entry, store, and repo materialization, and perform only `attached -> detached`.
* Preserve the observed strategy: a symlink remains a symlink and a copy remains a copy.
* Always preserve the exclude, regardless of symlink/copy strategy.
* Treat `--keep-ignore` as implicitly enabled when `--keep-store` is specified.
* Do not change content.

Document this as a safety-related contract correction to the existing default exclude removal and include it in migration notes. `--keep-ignore` remains valid for normal restore. Clearly describe detach semantics in CLI help and the reference so the command name is not mistaken for normal restore.

### `item move`

* Move only a managed symlink or equal regular copy.
* Reject a diverged regular copy and require an `item sync` with explicit direction first.
* Preserve the healthy observed strategy at the destination.
* If the source materialization is missing, reject and require `item repair` first.
* Add and verify the destination exclude before writing, and reject tracked, occupied, or unsafe paths.
* Treat updates to the store path, repo path, manifest, and excludes as a plan/phased operation with a durable operation record.
* Use the secure transfer helper for cross-device store moves.

### `item relink`

Keep relink separate from normal sync because it performs a `detached -> attached` ownership transition.

* Add and verify the exclude entry before attach. Do not require the user to run `repo repair` because the exclude is missing.
* Missing repo path: materialize with the configured strategy, then attach.
* Valid symlink or equal regular copy: preserve the observed strategy and attach.
* Reject unsafe, unexpected, or unsupported entries.
* Reject a diverged regular copy by default.

Detached items are not eligible for `sync --from repo`, so only relink itself provides an explicit direction when a diverged copy must be resolved.

```sh
shelfbox item relink <PATH> --from store [--dry-run]
shelfbox item relink <PATH> --from repo [--dry-run] --yes
```

`--from store` atomically replaces the repo copy before attach. `--from repo` atomically replaces the store before attach. Require `--yes` for actual `--from repo` writes, but not for `--dry-run`.

Both directions reject tracked state, a missing store, and path/file-identity violations. Add and verify the exclude first, then execute durably according to the directional relink recovery truth table. When equal content is relinked without a direction, attach as before; this normal relink is outside the durable-record scope.

## Relationship to Existing Features

### `repo reclaim`

Preserve existing ownership decisions regardless of copy/symlink strategy. Reclaim itself does not convert materializations. After reclaim, `repo repair` restores a missing materialization with the configured strategy.

### `store gc` / `rebuild-index`

No change. Do not use repo-copy presence or divergence in deletion decisions. When an unfinished operation exists, GC does not make related paths deletion candidates and requires recovery.

### `store verify`

Do not reduce its existing scope.

* Verify the presence and safety of every manifest and canonical store file.
* When an index entry has an associated local repo, also verify repo-side materialization through facts/policy.
* Support both symlinks and copies, reporting copy divergence and exclude/Git state.
* Continue checking the canonical store when a local repo entry is unavailable.
* Use the shared status evaluator; do not duplicate decisions.
* Preserve separate `WARNING` and `ERROR` labels in CLI output. Either label
  returns exit code `2` for `store verify`.

## v0.9.0 Implementation Scope

### Required

1. Config `materialization` and `MaterializationStrategy`
2. Facts model, policy evaluator, and copy-aware `Materializer`
3. Secure transfer, cross-device transfer, and platform filesystem adapter
4. Copy-aware `item add`
5. `item status` / `repo status` with severity/issues and schema-v2 fields
6. Copy-aware `item repair` / `repo repair`
7. `item sync --from store|repo`
8. Copy support and divergence guards for existing `item restore`, `item move`, and `item relink`
9. Limited durable operation recovery and phase truth tables for add, restore, move, and directional relink
10. Copy-aware `store verify`
11. Updated CLI text/JSON reports, exit codes, documentation, and migration notes
12. Failpoint, cross-device, and platform-specific integration tests

## Deferred to v0.9.1

`item materialize` and repo-level operations are separated from v0.9.0 scope, but will be implemented in v0.9.1 as follow-up features that complete copy-mode operation.

### `item materialize`

Add explicit strategy conversion.

```sh
shelfbox item materialize <PATH> --strategy copy [--dry-run]
shelfbox item materialize <PATH> --strategy symlink [--dry-run]
```

* Symlink -> copy: create a temp copy from the store.
* Equal copy -> symlink: create a temp symlink.
* Diverged copy -> symlink: reject and require `item sync` with explicit direction first.
* If target strategy matches observed materialization, no-op.
* Do not change manifest identity or `ownership_state`.
* Reject tracked state, missing exclude, missing store, and unsafe/unexpected entries.

Do not delete the existing materialization first.

```text
create temp copy / temp symlink from store in the same directory
-> verify new materialization and file identity
-> atomically replace existing entry
```

On POSIX, use rename without following the target. Encapsulate Windows replacement semantics in the adapter; if a sharing violation or similar condition prevents preservation of the old entry, fail without converting it.

### Repo operations

* `repo sync --from store|repo`
* `repo materialize --strategy symlink|copy`
* Reuse item-level operation conflict policy, reports, and recovery.
* For `--from repo`, show the target list first and require `--yes` for actual writes.

### Performance and schema

The following are not committed v0.9.1 deliverables. Reassess their necessity using measurements from v0.9.0:

* accelerating content comparison with a hash cache
* whether per-item strategy persistence is needed
* whether backup retention/history is needed
* expanding operation coverage based on v0.9.0 recovery experience

### Re-evaluate the detach lifecycle

For compatibility, v0.9.0 preserves `restore --keep-store` as a legacy detach operation. In v0.9.1, evaluate usage and migration cost and compare:

* preserving the current name and semantics while clarifying documentation only
* adding `item detach` with the same semantics and gradually deprecating `restore --keep-store` as a compatibility alias
* if users need to restore a regular repo file while retaining the store, designing a separate command/option without changing the meaning of the existing flag

Do not change existing `restore --keep-store` into symlink -> regular-copy conversion without a migration path. Users who need strategy conversion should explicitly combine it with `item materialize`.

### v0.9.1 acceptance criteria

* `item materialize` does not delete the existing materialization first.
* Replacement failure leaves the old materialization usable.
* A diverged copy is not converted until a direction is selected.
* The atomic replacement policy for symlink <-> copy is satisfied on both POSIX and Windows.
* Repo sync/materialize reuse item-level validation, conflict policy, and typed reports.
* A partial repo-level operation failure does not overwrite unverified items or diverged content.
* Item/repo `--dry-run` changes none of filesystem, manifest, or exclude state.

## Test Plan

### Unit tests

* Config default, `symlink`, `copy`, and unknown values
* Every classification on each facts axis and inspection failures
* Policy table for facts -> severity/issues/operation decision
* Strategy difference remains Healthy / exit `0`
* Streaming equality independent of metadata
* Rejection of path escapes, followed final components, reparse points, hardlinks, and unsafe file identities
* Secure temp permissions, atomic replacement, and cross-device fallback
* Operation-record phase transitions and recovery decisions
* Every add/restore/move/directional-relink phase truth table uniquely classifies all states
* JSON schema v2, nullable copy link fields, and symlink field compatibility

### Integration tests

* After copy add, store and repo contain independent regular files with equal content.
* Exclude failure leaves the source file entirely unchanged.
* Recovery is safe from every add-phase failpoint.
* Forward recovery or rollback is safe from every restore/move/directional-relink phase failpoint.
* With an unfinished operation, the next mutation is rejected or recovered, while read-only status does not modify the record.
* Recovery stops without overwriting when the repo path changes after interruption.
* A secure temp never becomes group/world-readable during the operation.
* Add/move/restore work when repo and store are on different filesystems.
* After editing a copy, status reports the `content_diverged` error and repair
  is non-destructive.
* A copy with a missing exclude is an error; a symlink with a missing exclude is a warning.
* `sync --from store` rejects missing and updates only diverged copies.
* `sync --from repo --yes` updates store content while preserving store permissions.
* Sync rejects tracked state, missing exclude, missing store, and hardlinks.
* Item repair does not change excludes; repo repair restores excludes first.
* Repo repair excludes attached items and detached items with repo entries, and does not repair detached materializations.
* A detached item with no repo entry preserves an existing exclude but does not gain a new one.
* Wrong-target symlink `--force` remains compatible and regular files are not overwritten.
* `restore --keep-store` preserves observed strategy and exclude and can be relinked directly.
* Relink safely adds a missing exclude before attach.
* Restore, move, and relink handle equal/diverged copies according to policy.
* A config-only change does not change existing materializations.
* A mixed symlink/copy repo is Healthy, with strategy differences reported only as notes.
* Store verify checks the canonical store and symlink/copy materializations in associated repos.
* Existing GC/rebuild-index/reclaim behavior remains unchanged.
* `--dry-run` changes none of filesystem, manifest, exclude, or operation-record state.
* Text/JSON output and `0/1/2` exit codes remain correct.

Copy tests must run in CI environments without symlink support, while existing symlink tests continue on supported platforms. On Windows, dedicated tests cover reparse points, replacement, sharing violations, and cross-volume behavior.

## Release Criteria

Do not release copy mode until:

* no ordinary add failure point leaves an unexcluded regular copy
* every crash point in add/restore/move/directional relink produces either recovery according to its truth table or a non-destructive conflict report
* repair, restore, and move never implicitly destroy a diverged copy
* writing back to the store requires explicit direction and confirmation
* a copy with a missing exclude is an error, and all content/materialization writes other than exclude repair are rejected
* status distinguishes warnings from errors and returns causes as schema-v2 typed issues
* a difference between configured and observed strategy alone is not a warning
* config changes do not implicitly convert existing items
* secure-temp and replacement safety requirements are satisfied across devices and on Windows
* existing local integration checks in `store verify` are preserved
* existing symlink-mode integration tests and JSON field values pass without regression

## Positioning

```text
symlink mode:
  store file is directly visible through repo path.

copy mode:
  store file is canonical.
  repo file is an editable materialized copy.
  repo edits become canonical only after an explicit --from repo operation.
```

As long as this boundary is maintained, copy mode can be introduced as an optional materialization for restricted environments without changing the meaning of ownership, repair, reclaim, or GC.
