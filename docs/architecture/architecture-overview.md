# Architecture Overview

This document provides a high-level overview of the shelfbox architecture.

For a module-by-module boundary map, see `module-map.md`.

For detailed storage structures, see `data-model.md`.

For individual design decisions and rationale, see `design-decisions.md`.

For the copy-materialization contract and its required
architecture boundaries, see [`../spec/copy-mode.md`](../spec/copy-mode.md).

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

* Public operational facade in `shelfbox_core::api`
* Domain data models
* Operation plan and report types
* Store management
* Ownership management
* Repair and recovery operations
* Private Git, filesystem, storage, and policy implementation modules

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

All operational behavior is delegated to `shelfbox_core::api`. Command handlers
live in `crates/shelfbox/src/commands/`.

---

# Core Subsystems

## Public API Facade

Responsible for:

* Stable operation groups for `config`, `item`, `repo`, and `store`
* Construction of default adapters used by the CLI
* Returning structured reports and plans to presentation layers
* Re-exporting intentional public types from lower layers

Downstream callers should use `shelfbox_core::api` rather than importing
implementation modules directly.

---

## Context

Responsible for:

* Repository discovery
* Store discovery
* Configuration loading
* Lock acquisition
* Manifest loading

Most item and repository operations begin by building a context. Commands such
as `repo reclaim` first inspect the current Git checkout without creating a new
`RepoId`, then build or update context only after explicit user intent.

`context` is a crate-private implementation module. Public context-building
entry points are exposed through `shelfbox_core::api::item` and
`shelfbox_core::api::repo`.

---

## Store

Responsible for:

* Repository registration
* Item storage
* Manifest persistence
* Store integrity
* Strategy-neutral materialization of repo paths (default symlink or Copy mode)

The store is the source of truth.

Storage implementation modules are crate-private. Persistent data shapes that
are part of the public contract live under `shelfbox_core::domain`, and
operation reports live under `shelfbox_core::plan`.

---

## Policy

Responsible for:

* Safety and eligibility decisions
* Ownership transition constraints
* Path escape checks
* GC, migration, reclaim, and repair rules

Policy modules are pure decision layers over already-collected facts. They do
not perform filesystem, Git, or storage I/O.

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
shelfbox_core::api
 ↓
Private context / operation / policy layers
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
best-effort reclaim hint check
 ↓
shelfbox_core::api::item::build_create_or_load()
 ↓
shelfbox_core::api::item::add_file()
 ↓
manifest save
 ↓
output
```

The `materialization` configuration key is a default for new or missing
repo-side entries. It is intentionally not persisted per item: operations
inspect the observed symlink or regular copy before acting.

Detailed flow for `shelfbox item add <PATH>`:

```text
cli::run()
  -> commands::item::run_item()
      - optionally print explicit reclaim hint for positive candidates
  -> shelfbox_core::api::item::build_create_or_load(cwd, store_override)
      - discover repository root
      - load resolved configuration
      - load/upsert store index
  -> shelfbox_core::api::item::add_file(...)
      - validate target path
      - move file to store
      - create the configured repo-side materialization
      - persist manifest
      - update .git/info/exclude
```

Detailed flow for `shelfbox repo repair`:

```text
commands::repo::run_repo()
  -> shelfbox_core::api::repo::current_git_context(...)
      without creating a new RepoId
  -> resolve existing association from index.json
  -> shelfbox_core::api::repo::build_create_or_load(...)
      after association is proven, build a write-capable context for repair
  -> shelfbox_core::api::repo::repair_repo(...)
      - restore missing exclude entries
      - repair missing materializations
      - refresh local Git metadata in index.json
      - refresh identity hints
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

## Repair vs Reclaim

Repair restores integrity.

Reclaim associates the current Git clone with an existing `RepoId`.

Repair must never perform reclaim.

Reclaim is only performed by explicit user action:

```text
repo reclaim
```

---

# Related Documents

* `data-model.md`
* `design-decisions.md`
* `spec/ownership-model.md`
* `spec/failure-matrix.md`
