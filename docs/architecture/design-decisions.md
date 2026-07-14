# Design decisions

| Decision | Rationale |
|---|---|
| **No `git2` dependency** | `std::process::Command` is sufficient for the Git queries shelfbox needs. Avoiding `git2` keeps compile times shorter and avoids a large C dependency. |
| **ULID for repository and item IDs** | ULIDs are sortable, URL-safe, collision-resistant, and human-readable enough for CLI diagnostics without requiring a database. |
| **`repos/` is the canonical persistent store** | Preserving `repos/` must be enough to recover managed data. `index.json` and `meta.json` are local metadata/cache files and must be safely regenerable. |
| **`RepoId` is the only repository identity** | Repository store directory names, Git remotes, Git paths, and hostnames are not identity. This keeps recovery explicit and avoids accidental merges between clones. |
| **Human-readable repo store directories** | New repository store directories use `<name>`, then `<name>-2`, `<name>-3`, etc. The name is only a locator; the stable identity is the `repo_id` in `manifest.json`. |
| **Atomic index and manifest writes** | `index.json` and `manifest.json` are written with a temp-file-then-rename strategy so a crash mid-write cannot leave partial JSON. |
| **`store_path` is repo-store-relative** | Relative paths such as `items/.env` keep `manifest.json` portable across machines and store relocations. |
| **No absolute paths in `manifest.json`** | Absolute paths are local machine cache data. They belong in `index.json` and may be absent after `store rebuild-index`. |
| **Identity hints are hints only** | `remote_hints`, `repo_name_hints`, and `last_attached_at` are used for display, candidate ranking, and best-effort reclaim hints. They are never proof of identity and never trigger automatic reclaim. |
| **Remote hints use host/path format** | Remote URLs are normalized to `host/org/repo` style before entering `remote_hints` (for example `github.com/org/repo`). Scheme, user, query, fragment, and `.git` suffixes are discarded; local/file URLs are not stored as remote hints. |
| **Explicit reclaim instead of automatic identity detection** | A new clone receives a new `RepoId` unless the user explicitly runs `repo reclaim`. This prevents remote URL or name matches from silently merging unrelated repositories. |
| **Reclaim does not transfer ownership** | `repo reclaim` associates the current clone with an existing `RepoId`; it does not move items, copy items, change ownership state, repair materializations, or rewrite excludes. |
| **Repair is ownership-neutral** | `repo repair` and `item repair` restore local integration but never assign a different `RepoId` or change item ownership state. |
| **Conservative GC** | GC may delete only confirmed `orphaned` items. `attached`, `detached`, and `unreachable` items are protected. Manifest entries are removed and saved before store files are deleted, so a manifest-save failure does not remove data. Repository store directories are not deleted merely because a local clone is missing. |
| **Namespace is UI only** | Directory grouping is derived from `item.path`. Namespace entries are not persisted as identity, ownership, recovery, reclaim, repair, or GC metadata. |
| **Configured strategy is a default, observed strategy is runtime state** | `materialization = symlink|copy` selects only future or missing repo-side entries. The manifest stores no per-item strategy; operations inspect the actual symlink or regular copy, so changing config never converts an existing item. `LinkStrategy` remains the low-level symlink adapter. |
| **`# BEGIN shelfbox` block in exclude** | All shelfbox entries are wrapped in a named block so other tools can safely edit `.git/info/exclude`; content outside the block is preserved. |
| **Store-level advisory file lock** | Repo-context operations acquire `<store>/.lock` so ordinary item and repository writes do not interleave index and manifest updates. |
| **Machine-readable exit codes** | Status and verify commands return stable process codes so they can be used in scripts and CI. |
| **Explicit manifest migration** | Legacy manifests are upgraded only by `store migrate-manifests`; normal commands reject unsupported versions instead of silently rewriting canonical data. |
| **`SHELFBOX_STORE` environment variable** | The store root can be selected by environment variable, with precedence below `--store` and above config/default paths. |
| **Private, fail-closed platform filesystem adapter** | No-follow inspection, stable identity, link counts, replacement, and directory durability are platform capabilities behind `fs::platform`. Operations never call OS APIs directly, and no unsupported guarantee falls back to delete-then-create. |
| **SHA-256 recovery fingerprints** | Durable operation records use a bounded-memory SHA-256 content fingerprint serialized as `{ "algorithm": "sha256", "digest_hex": "<64 lowercase hex>" }`. This is recovery safety metadata, not a routine status hash cache. |
| **Option-driven durable atomic writes** | `storage::atomic_write` creates same-directory temp files with `create_new`, can fsync file content before rename, uses the platform atomic-replace adapter, and can require or best-effort parent-directory fsync. Generated temp files are the default; fixed temp paths are opt-in and never overwritten. |
| **Status schema v2 is additive** | Existing Rust `ItemStatus` and `IntegrityReport` remain literal symlink-compatibility projections. CLI JSON and copy-aware callers use option-bearing v2 APIs with `status_schema_version = 2`, generic materialization fields, stable snake_case codes, and nullable legacy link fields for copy items. |
| **Copy mutations require artifact leases** | No copy mutation may write plaintext before its temp path is durably recorded, repo-side temps are exactly excluded, and the empty temp identity is durably recorded. Commit must be bracketed by pre/post validation, and cleanup may remove only matching artifacts. |
| **Operations use materializer and canonical-transfer ports** | Operations issue typed actions, persist high-level phases, and request opaque commit permits. Filesystem adapters own symlink/copy dispatch, no-follow facts, temp artifacts, and transfer algorithms; canonical store movement is a separate port. |

## D1: Platform filesystem adapter

Copy mode requires stronger guarantees than `std::fs` exposes uniformly. The
platform adapter is therefore private to `shelfbox-core::fs`; higher layers see
typed facts and capability failures rather than OS flags, handles, or errno
values.

### Capability matrix

`Runtime checked` means the platform API has the required semantics, but the
mounted filesystem may reject it. Rejection is an error and never selects a
weaker algorithm.

| Capability | Linux | macOS | Windows |
|---|---|---|---|
| Final-component no-follow open and metadata | Supported with `O_PATH | O_NOFOLLOW`, then handle metadata | Supported with `O_SYMLINK | O_NOFOLLOW`, then handle metadata | Supported with `FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS`, then handle metadata |
| Stable file identity | `(st_dev, st_ino)` from the opened handle | `(st_dev, st_ino)` from the opened handle | Runtime checked: volume serial plus 128-bit `FILE_ID_INFO` from the opened handle |
| Link count | `st_nlink` from the opened handle | `st_nlink` from the opened handle | `nNumberOfLinks` from `BY_HANDLE_FILE_INFORMATION` |
| Replace an existing regular file | Same-directory `rename`, which atomically replaces the destination | Same-directory `rename`, which atomically replaces the destination | Runtime checked: handle-based `SetFileInformationByHandle(FileRenameInfo)` with `ReplaceIfExists` |
| Replace a symlink/reparse point without following it | `rename` replaces the symlink entry | `rename` replaces the symlink entry | Runtime checked: the same handle-based rename replaces the reparse-point entry; contract tests verify the target remains unchanged |
| Destination sharing behavior | Open handles do not normally block same-filesystem rename | Open handles do not normally block same-filesystem rename | A destination handle that denies delete sharing produces a sharing violation; source and destination must remain unchanged |
| Parent-directory durability after rename | Runtime checked by opening the directory without following its final component and calling `fsync` | Runtime checked by opening the directory without following its final component and calling `fsync` | Unsupported as a separate capability: `FlushFileBuffers` documents file-buffer flushing but no parent-directory fsync equivalent |

The no-follow primitive covers the final component. Phase 2 must additionally
walk or guard parent components beneath already validated repo/store roots. On
Windows, parent handles must be retained without delete sharing across the
prepare/commit boundary and their identities revalidated; if that cannot be
established for a path/filesystem pair, mutation returns a typed
`FilesystemCapabilityUnavailable` error.

### Dependency decision

Use target-specific dependencies only:

* `libc` on Unix for `O_PATH`, `O_SYMLINK`, `O_NOFOLLOW`, `O_DIRECTORY`, and
  descriptor-level opens.
* `windows-sys` on Windows for the narrow Win32 handle-information and
  handle-rename surface.

Do not add handwritten Windows FFI, a broad cross-platform filesystem crate,
or OS-specific calls under `ops/`. Concrete adapters remain constructible only
inside the filesystem layer/composition root.

`ReplaceFileW` is intentionally rejected for the commit primitive. Microsoft
documents partial-failure outcomes in which the old destination may no longer
retain its original name. That violates shelfbox's old-destination preservation
invariant. `SetFileInformationByHandle(FileRenameInfo)` performs the rename
from an already opened source handle and fails on incompatible destination
sharing; there is no delete-then-create fallback.

### Capability failures

`FilesystemCapability` identifies the required guarantee, and
`AppError::FilesystemCapabilityUnavailable` reports the platform and reason.
Unsupported or runtime-rejected no-follow inspection, stable identity,
replacement, or durability stops the operation without changing the source or
destination.

### Primary references

* [Linux `rename(2)`](https://man7.org/linux/man-pages/man2/rename.2.html)
* [Apple `rename(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/rename.2.html)
* [Rust Unix `OpenOptionsExt`](https://doc.rust-lang.org/std/os/unix/fs/trait.OpenOptionsExt.html)
* [Microsoft `CreateFileW`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)
* [Microsoft `FILE_ID_INFO`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_info)
* [Microsoft `FILE_RENAME_INFO`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_rename_info)
* [Microsoft `SetFileInformationByHandle`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfileinformationbyhandle)
* [Microsoft `ReplaceFileW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew)
* [Microsoft `FlushFileBuffers`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers)

## D2: Recovery fingerprint

Durable operation records need a content fact that remains meaningful after a
crash. File identity alone is not enough because a file can be modified
in-place while retaining the same identity. Size and mtime are also not enough:
same-size rewrites are common, and mtime can change as an operation side effect.

Use SHA-256 as the v0.9.0 recovery safety fingerprint:

```json
{
  "algorithm": "sha256",
  "digest_hex": "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
}
```

The serialized contract is:

* `algorithm` is the lowercase string `sha256`.
* `digest_hex` is exactly 64 lowercase hexadecimal characters.
* Deserialization rejects unknown algorithms, short/long digests, uppercase
  hex, and non-hex characters.
* The digest may be stored in a recovery record; source content must never be
  stored in the record.
* The algorithm field is part of every serialized value so future algorithms can
  be introduced without changing how unfinished SHA-256 records are interpreted.

The implementation is `domain::recovery_fingerprint::RecoveryFingerprint`.
It streams input through a fixed 64 KiB buffer, so memory use is bounded and
independent of file size. The public constructor accepts only canonical
lowercase SHA-256 hex, and file calculation maps I/O failures into `AppError`
with the path being fingerprinted.

### Scope boundary

This digest is only for durable recovery decisions. Routine status checks should
continue to use streaming equality when they need content comparison. A deferred
status hash cache would need its own invalidation, persistence, and migration
design, and must not silently reuse recovery-record semantics.

### Dependency decision

Use the narrow RustCrypto `sha2` crate from the workspace dependency table.
Do not add a broad content-addressed-storage or checksum framework for D2. The
lowercase hex encoding is implemented locally to avoid adding a dependency for
64 bytes of stable formatting.

### Compatibility tests

Focused tests lock:

* the SHA-256 `abc` known vector;
* the exact JSON shape and lowercase-hex digest field name;
* rejection of unknown algorithms and non-canonical digest strings;
* changed fingerprints for same-size file rewrites; and
* chunking-independent streaming over data larger than the fixed buffer.

## D3: Atomic write durability

`storage::atomic_write` is the single crate-private helper for same-directory
temp-file writes followed by atomic replacement. Callers keep the existing
simple `write(path, contents, ParentDirMode)` entry point, while operation
records and secret-copy paths can opt into stricter durability through
`AtomicWriteOptions`.

### Write contract

The helper now supports:

* generated temp paths by default, named as a hidden same-directory file with a
  fresh ULID suffix;
* caller-provided temp paths only when explicitly requested;
* `create_new` temp creation, so an existing temp file is never overwritten;
* private temp file creation (`0600` on Unix) when the parent directory mode is
  private;
* optional `File::sync_all()` after writing and before replacement;
* atomic replacement through `fs::platform::atomic_replace`, preserving the
  D1 no-delete-then-create invariant;
* optional parent-directory durability with `Skip`, `BestEffort`, or `Require`;
  and
* cleanup that removes a temp path only when no-follow inspection shows the same
  file identity that this write created.

Production config, index, manifest, and meta writes use generated temp paths.
The previous predictable `.tmp` style is no longer used by production writers.
The fixed-temp helper remains available only as an explicit opt-in for callers
that have already reserved a same-directory temp path.

### Durability modes

`ParentDirectorySyncMode::Require` preflights the parent-directory sync before
creating a temp file. On platforms where D1 marks directory durability
unsupported, this fails closed before mutating the destination. After a
successful replacement, `Require` syncs the parent again and returns any error;
`BestEffort` attempts the sync and ignores failure; `Skip` preserves the legacy
rename-only behavior.

File fsync and parent-directory fsync are separate choices because operation
records need both, while some existing metadata writes may remain rename-only
until their durability requirements are upgraded deliberately.

### Cleanup and failure behavior

Generated and fixed temp files are opened with `create_new`; if a fixed temp
path already exists, the write fails and leaves that path untouched. On write,
fsync, or replacement failure, the cleanup guard closes the temp handle, checks
the current temp path with no-follow identity inspection, and removes it only if
the identity still matches the file created by this write.

Replacement failures preserve the old destination and remove only the owned temp
file. If a required parent-directory sync fails after a successful replacement,
the destination has already been updated; callers that require full durability
must report that error and rely on recovery records for subsequent reconciliation.

### Compatibility tests

Focused tests lock:

* generated temp writes replace content and leave no temp files behind;
* fixed temp paths are same-directory, create-new, and never overwrite an
  existing file;
* failed replacement preserves the destination and cleans the generated temp;
* private temp creation yields a private destination file on Unix;
* file fsync plus required parent-directory fsync succeeds on Unix; and
* required parent sync fails before mutation on Windows.

## D4: JSON status schema representation

Status has two compatibility layers:

* Legacy Rust/JSON status remains the v0.8.0 symlink shape:
  `path`, `link_exists`, `link_valid`, `store_exists`, `in_exclude`,
  `not_tracked`, and `ok`.
* Schema v2 is the copy-aware additive shape. It is exposed through
  option-bearing APIs and is the JSON shape emitted by the CLI status
  formatter.

The existing public `ItemStatus`, `IntegrityReport`, `item::status`, and
`repo::integrity_check` APIs remain source-compatible. They must continue to be
literal symlink projections and must not reinterpret `link_exists` or
`link_valid` as generic materialization booleans.

### Schema-v2 item contract

Each v2 item includes:

```rust
struct ItemStatusV2 {
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

Serialization rules:

* `status_schema_version` is always `2`.
* All enum values serialize as snake_case.
* `ok` is equivalent to `severity == "healthy"`.
* `issues[].code` and `notes[].code` are stable machine-readable codes.
* Symlink items keep `link_exists` and `link_valid` as their previous boolean
  values.
* Copy items serialize `link_exists` and `link_valid` as `null`.
* The `item status` v2 outer shape remains a JSON array.
* The `repo status` v2 outer shape remains an object with `items`,
  `orphan_store_items`, and `repo_root_matches_index`.

Current v2 enum vocabularies are:

* `StatusSeverity`: `healthy`, `warning`, `error`
* `MaterializationStrategy`: `symlink`, `copy`
* `ObservedMaterialization`: `missing`, `managed_symlink`,
  `unmanaged_symlink`, `regular_file`, `directory`, `other`
* `CopyContentState`: `not_applicable`, `equal`, `diverged`,
  `store_missing`, `repo_unreadable`, `store_unreadable`, `unknown`
* `StatusIssueCode`: `materialization_missing`,
  `materialization_invalid`, `store_missing`, `missing_exclude`,
  `tracked_by_git`, `content_diverged`, `content_unreadable`,
  `hardlink_unsafe`, `path_escape`, `unfinished_operation_conflict`
* `StatusNoteCode`: `strategy_mismatch`

### API boundary

The public copy-aware APIs are:

* `item::status_v2(ctx, StatusOptions::v2()) -> Vec<ItemStatusV2>`
* `repo::integrity_check_v2(ctx, StatusOptions::v2()) -> IntegrityReportV2`

Legacy and v2 projections are derived from the same collected facts. The
legacy projection remains suitable for existing symlink consumers; copy-aware
callers must use v2.

### Compatibility tests

Focused tests lock:

* schema-v2 item JSON for a healthy symlink;
* schema-v2 item JSON for a copy item, including `null` legacy link fields;
* schema-v2 repo JSON outer shape;
* snake_case enum, issue-code, and note-code serialization; and
* compile-time source compatibility for existing legacy APIs and new v2 APIs.

## D5: Copy mutation crash-safety contract

Copy mutation code must not begin until the temp/artifact protocol below is
available through a mutation journal and opaque artifact leases. The contract is
encoded in `domain::copy_safety` as crate-private constants with focused tests;
the module is a contract fixture, not a mutation implementation.

### Repo-side temp protocol

Repo-side temps can become Git-visible plaintext if the order is wrong. Their
protocol is:

1. Generate a temp path without creating the file.
2. Durably record the artifact path.
3. Add and verify the exact managed exclude for that temp path.
4. Create an empty private temp with create-new semantics.
5. Capture and durably record the temp identity.
6. Authorize plaintext writing.
7. Revalidate immediately before commit.
8. Atomically replace the destination.
9. Revalidate repo Git/exclude state after materialization.
10. Remove the temp-specific exclude and artifact record only after final state
    is verified.

The important write-ahead barriers are:

* artifact path before file creation;
* exact repo-temp exclude before file creation;
* temp identity before plaintext writing; and
* final verification before artifact cleanup.

### Store-side temp protocol

Store-side temps do not need Git excludes, but they still need artifact records
and identity-safe cleanup:

1. Generate a temp path without creating the file.
2. Durably record the artifact path.
3. Create an empty private temp with create-new semantics.
4. Capture and durably record the temp identity.
5. Authorize plaintext writing.
6. Revalidate immediately before commit.
7. Atomically replace the destination.
8. Revalidate after materialization.
9. Remove the artifact record only after final state is verified.

### Boundary ownership

The mutation journal / artifact lease owns:

* temp path allocation;
* durable artifact path recording;
* repo-side exact temp exclude creation and verification;
* empty private temp creation;
* durable temp identity recording;
* writable-temp authorization; and
* identity-safe artifact cleanup.

`Materializer` or `CanonicalTransfer` owns:

* populating the prepared artifact; and
* atomically committing the prepared artifact.

The operation owns:

* high-level durable phase updates;
* obtaining a fresh `WritePreconditionGuard`;
* post-materialization validation; and
* recovery direction decisions.

Operations must not learn temp paths, temp identities, transfer algorithms, or
symlink/copy implementation state from prepared handles.

### WritePreconditionGuard checklist

Immediately before commit, `WritePreconditionGuard` rechecks:

* repo/store root containment;
* parent no-follow conditions;
* final-component no-follow conditions;
* file identity;
* link count and hardlink absence;
* Git tracked state;
* target exclude state;
* all excludes owned by artifact leases; and
* destination equality with the planned state.

Because Git and exclude updates cannot be transacted with filesystem
replacement, operations must also perform post-materialization validation. On
post-check failure, rollback is allowed only when the recorded identity still
matches; otherwise preserve artifacts and report a conflict.

### Failpoint and recovery rules

Every operation without a full durable operation record needs failpoint proof
after each persistent mutation:

* artifact path record;
* repo-temp exclude;
* empty temp creation;
* temp identity record;
* plaintext write;
* destination replacement;
* post-materialization validation record; and
* artifact record deletion.

Any operation that cannot prove safe retry after each mutation must be promoted
to a full durable operation record or recovery-artifact record before release.

Stale-completion recovery is:

* if final postconditions already hold when an unfinished record reappears,
  classify the operation as completed;
* remove only the stale record and matching artifacts;
* never roll back completed user-visible state; and
* write artifact and backup paths to the record before creating them.

### Contract tests

Focused tests lock:

* repo-side ordering from path generation through plaintext authorization;
* store-side ordering without repo excludes;
* pre/post validation around atomic replacement and cleanup;
* the full `WritePreconditionGuard` checklist;
* failpoint proof after every persistent mutation;
* ownership boundaries that keep temp details opaque to operations; and
* stale-completion recovery rules.

## D6: Operation/materializer boundary

D6 freezes crate-private operation-facing ports before any copy-aware operation
is migrated. These are contracts and fake-adapter prototypes, not a copy
materialization implementation. The concrete Phase 3 adapters will live under
`fs` and be constructed only by the API/context composition root.

### Typed actions and facts

`fs::materializer::MaterializationAction` is the only repository
materialization mutation vocabulary:

* `NoOp`;
* `Create { location, strategy }`;
* `Replace { location, strategy, expected }`;
* `Remove { location, expected }`; and
* `RestoreToRegular { location, expected }`.

`location` consists only of normalized repo-relative and store-relative paths.
`expected` carries a visible high-level entry kind plus a private identity
snapshot. A `Materializer::inspect` result similarly exposes policy-relevant
no-follow facts—entry kind, final-component inspection state, link count, and
hardlink safety—without exposing raw file identities or platform handles.
Operations obtain `ExpectedMaterialization` from those facts rather than
constructing an identity precondition themselves.

`Materializer` has four methods: read-only `inspect`, `prepare`, `commit`, and
`abort`. It owns symlink/copy dispatch, platform inspection, secure transfer,
and artifact population. It does not own Git/exclude policy, confirmation,
manifest ownership, durable operation direction, or user-facing reports.

Canonical store movement uses the distinct
`fs::canonical_transfer::CanonicalTransfer` port. Its `Move` and
`ReplaceFromRepo` actions name logical canonical endpoints and expected state,
but never choose rename, copy, or cross-device transfer algorithms. It has the
same inspect/prepare/commit/abort lifecycle as `Materializer`.

### Opaque journal, handles, and permits

`MutationJournal` is the only D6 interface for artifact lifecycle steps. A
materializer or canonical-transfer adapter obtains an `ArtifactLease` for the
appropriate scope, then asks the journal to authorize a plaintext write. The
journal performs the D5 write-ahead barriers before that authorization:
artifact recording, repo-temp exclude when applicable, private create-new,
and temp-identity recording. The writable lease, prepared handle, lease
reference, commit context, and commit permit intentionally have no path,
identity, or transfer-algorithm accessors.

The required operation sequence is:

1. Pass the policy-approved typed action to `prepare` with an operation-scoped
   journal.
2. Persist the high-level `*Prepared` durable phase.
3. Request fresh no-follow facts through the relevant port.
4. Combine those facts with the opaque prepared commit context to create a
   `WritePreconditionGuard`, perform the Git/exclude checks, and obtain a
   `CommitPermit` from the journal.
5. Persist `CommitAuthorized`, commit the opaque prepared handle, and persist
   the corresponding `*Committed` phase.
6. Inspect and validate postconditions, then persist `PostCommitValidated`.

Thus `WritePreconditionGuard` gets identity, final-component no-follow, link
count, and hardlink facts only from port inspection. Operations do not import
`fs::platform`, inspect raw handles, or learn temp paths. The guard retains the
full D5 checklist: containment, no-follow conditions, identity/link-count,
Git/exclude state including artifact leases, and planned destination equality.

### Dependency enforcement

`crates/shelfbox-core/tests/architecture_boundaries.rs` is active before the
first copy-aware operation migration. It rejects production `ops/` references
to platform modules, secure transfer, symlink helpers, and platform-specific
symlink APIs. It also prevents `LinkStrategy` and direct copy/rename/removal/
read-link calls from spreading beyond the current symlink-only modules.

The following existing v0.8.0 modules are an explicit pre-migration baseline:
`add`, `info`, `integrity`, `move_item`, `relink`, `repair`, `restore`, and
`status`. Their total `LinkStrategy` references may decrease from the recorded
ceiling of 30 but may not increase or appear in a new production operation
module. Their narrowly enumerated direct filesystem calls are likewise
allowlisted only in the existing operation files. Each Phase 3 operation
migration must remove its legacy allowance; no copy-aware operation may use
one. This preserves existing symlink behavior while making the dependency
boundary enforceable now.

### Prototype tests

Focused operation tests use a fake `Materializer`, fake `CanonicalTransfer`,
and fake `MutationJournal`. They prove that operations request the exact typed
action, lease repo-side versus store-side artifacts correctly, record durable
phases around the commit permit, inspect before and after commit, and receive
no prepared-handle detail beyond an opaque commit context. The contracts are
platform-neutral; their implementations must satisfy the D1 capability matrix
on Linux, macOS, and Windows where the required capability applies.
