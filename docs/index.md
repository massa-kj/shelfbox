# Documentation Index

This directory contains all project documentation.

Choose a document based on what you want to do.

## Getting Started

### [Getting Started](./getting-started.md)

Read this first if you are new to shelfbox.

Covers:

* What shelfbox is
* Installation
* Basic usage
* First shelved item

---

## Common Workflows

### [Common Workflows](./workflows.md)

Task-oriented guides for common operations.

Examples:

* Keep a `.env` file out of Git
* Move the store to a new location
* Recover after a repository move
* Recover after a reclone
* Repair broken symlinks
* Rebuild a lost manifest

Use this document when you know what you want to achieve but do not yet know which command to use.

---

## Command Reference

### [Item Commands](./reference/item-commands.md)

Reference for:

* `item add`
* `item restore`
* `item repair`
* `item relink`
* `item move`
* `item list`
* `item status`
* `item info`

### [Repo Commands](./reference/repo-commands.md)

Reference for:

* `repo list`
* `repo status`
* `repo reclaim`
* `repo repair`
* `repo gc`

### [Store Commands](./reference/store-commands.md)

Reference for:

* `store info`
* `store verify`
* `store rebuild-index`
* `store migrate-manifests`
* `store gc`

### [Config Commands](./reference/config-commands.md)

Reference for:

* `config list`
* `config get`
* `config set`
* `config explain`
* `config path`

Use the reference documents when you already know the command and need exact behavior, flags, outputs, or error conditions.

---

## Architecture

### [Architecture Overview](./architecture/architecture-overview.md)

High-level system architecture.

Covers:

* Workspace structure
* Crate responsibilities
* Request lifecycle
* Major subsystems

### [Data Model](./architecture/data-model.md)

Persistent data structures and storage layout.

Covers:

* `meta.json`
* `index.json`
* `manifest.json`
* Store directory structure

### [Design Decisions](./architecture/design-decisions.md)

Important design decisions and their rationale.

Examples:

* Ownership model choices
* Store layout decisions
* Repair policy
* Repository identity strategy
* Concurrency and locking

Use the architecture documents when modifying implementation or evaluating design changes.

---

## Specifications

### [Ownership Model](./spec/ownership-model.md)

Formal ownership state machine specification.

Defines:

* Ownership states
* State transitions
* Command authority rules
* Ownership invariants
* Reclaimability rules

This is the authoritative source for ownership semantics.

### [Failure Matrix](./spec/failure-matrix.md)

Operational failure and recovery specification.

Defines:

* Failure modes
* Detection methods
* Recovery procedures
* Recoverability guarantees
* Recovery invariants

Use this document when evaluating recovery behavior or failure handling.

---

## Reading Guide

| Goal                           | Document                  |
| ------------------------------ | ------------------------- |
| Learn shelfbox                 | [`getting-started.md`](./getting-started.md) |
| Solve a practical problem      | [`workflows.md`](./workflows.md) |
| Understand a command           | [`reference/*`](#command-reference) |
| Modify implementation          | [`architecture/*`](#architecture) |
| Understand ownership behavior  | [`spec/ownership-model.md`](./spec/ownership-model.md) |
| Understand recovery guarantees | [`spec/failure-matrix.md`](./spec/failure-matrix.md) |
