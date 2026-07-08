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
| **Reclaim does not transfer ownership** | `repo reclaim` associates the current clone with an existing `RepoId`; it does not move items, copy items, change ownership state, repair symlinks, or rewrite excludes. |
| **Repair is ownership-neutral** | `repo repair` and `item repair` restore local integration but never assign a different `RepoId` or change item ownership state. |
| **Conservative GC** | GC may delete only confirmed `orphaned` items. `attached`, `detached`, and `unreachable` items are protected. Manifest entries are removed and saved before store files are deleted, so a manifest-save failure does not remove data. Repository store directories are not deleted merely because a local clone is missing. |
| **Namespace is UI only** | Directory grouping is derived from `item.path`. Namespace entries are not persisted as identity, ownership, recovery, reclaim, repair, or GC metadata. |
| **Link strategy is runtime behavior** | `LinkStrategy` selects how links are created on the current platform. Per-item link metadata is not canonical manifest identity. If future hardlink/copy modes need persistence, they require a new versioned manifest field and migration. |
| **`# BEGIN shelfbox` block in exclude** | All shelfbox entries are wrapped in a named block so other tools can safely edit `.git/info/exclude`; content outside the block is preserved. |
| **Store-level advisory file lock** | Repo-context operations acquire `<store>/.lock` so ordinary item and repository writes do not interleave index and manifest updates. |
| **Machine-readable exit codes** | Status and verify commands return stable process codes so they can be used in scripts and CI. |
| **Explicit manifest migration** | Legacy manifests are upgraded only by `store migrate-manifests`; normal commands reject unsupported versions instead of silently rewriting canonical data. |
| **`SHELFBOX_STORE` environment variable** | The store root can be selected by environment variable, with precedence below `--store` and above config/default paths. |
| **Private, fail-closed platform filesystem adapter** | No-follow inspection, stable identity, link counts, replacement, and directory durability are platform capabilities behind `fs::platform`. Operations never call OS APIs directly, and no unsupported guarantee falls back to delete-then-create. |
| **SHA-256 recovery fingerprints** | Durable operation records use a bounded-memory SHA-256 content fingerprint serialized as `{ "algorithm": "sha256", "digest_hex": "<64 lowercase hex>" }`. This is recovery safety metadata, not a routine status hash cache. |

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

Use SHA-256 as the v0.8.1 recovery safety fingerprint:

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
