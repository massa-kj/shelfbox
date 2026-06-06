# Architecture Overview

This document provides a high-level overview of the shelfbox architecture.

For detailed storage structures, see `data-model.md`.

For individual design decisions and rationale, see `design-decisions.md`.

---

# Goals

The architecture is designed around the following principles:

* Simple and deterministic recovery
* No hidden databases
* Human-readable storage
* Explicit ownership tracking
* Clear separation between CLI and business logic

---

# Workspace Structure

`shelfbox` is implemented as a Cargo workspace.

```text
shelfbox/
├── Cargo.toml
└── crates/
    ├── shelfbox-core/
    └── shelfbox/
```

---

## shelfbox-core

The library crate.

Contains:

* Business logic
* Data models
* Store management
* Ownership management
* Repair and recovery operations
* Git integration

The library does not know about:

* CLI argument parsing
* Terminal formatting
* User-facing presentation

---

## shelfbox

The binary crate.

Contains:

* CLI definitions
* Command dispatch
* Output formatting
* Exit code handling

The binary does not implement business logic directly.

All operational behavior is delegated to `shelfbox-core`.

---

# Core Subsystems

## Context

Responsible for:

* Repository discovery
* Store discovery
* Configuration loading
* Lock acquisition
* Manifest loading

Every command begins by building a context.

---

## Store

Responsible for:

* Repository registration
* Item storage
* Manifest persistence
* Store integrity

The store is the source of truth.

---

## Ownership

Responsible for:

* Repository identities
* Item identities
* Ownership state transitions
* Reclaimability decisions

Ownership semantics are formally defined in:

```text
spec/ownership-model.md
```

---

## Integrity and Recovery

Responsible for:

* Status reporting
* Repair operations
* Manifest reconstruction
* Recovery workflows

Recovery behavior is formally defined in:

```text
spec/failure-matrix.md
```

---

# Request Flow

Typical command execution:

```text
CLI
 ↓
Argument parsing
 ↓
Context construction
 ↓
Business operation
 ↓
Manifest / Store update
 ↓
Output formatting
```

Example:

```text
shelfbox item add .env
```

```text
CLI
 ↓
context::build()
 ↓
ops::add()
 ↓
manifest save
 ↓
output
```

Detailed flow for `shelfbox item add <PATH>`:

```text
cli::run()
  -> cmd::item::run_item()
  -> context::build(cwd, store_override, write=true)
      - discover repository root
      - load resolved configuration
      - load/upsert store index
  -> ops::add::add(...)
      - validate target path
      - move file to store
      - create repo-side symlink
      - persist manifest
      - update .git/info/exclude
```

Detailed flow for `shelfbox repo repair`:

```text
cmd::repo::run_repo()
  -> context::build(..., write=true)
  -> detect ownership transitions (attached -> stale/unreachable)
  -> ops::integrity::fix(...)
      - fix index root mismatch
      - rebuild manifest from deterministic store layout when needed
      - restore missing exclude entries
      - repair broken symlinks
  -> print per-fix results
```

---

# Design Boundaries

The following boundaries are intentional.

## Ownership vs Integrity

Ownership answers:

```text
Who owns this item?
```

Integrity answers:

```text
Is this item healthy?
```

The two concerns are independent.

---

## Identity vs Content

Ownership identity is not content identity.

Two identical files may have different item identities.

See:

```text
spec/ownership-model.md
```

---

## Repair vs Transfer

Repair restores integrity.

Transfer changes ownership.

Repair must never transfer ownership.

Ownership transfer is only performed by:

```text
repo adopt
```

---

# Related Documents

* `data-model.md`
* `design-decisions.md`
* `spec/ownership-model.md`
* `spec/failure-matrix.md`
