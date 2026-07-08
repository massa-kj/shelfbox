# Module Map

This map describes the v0.8.0 crate and module boundaries.

## Workspace

```text
crates/shelfbox
  CLI parsing, command dispatch, terminal formatting, and exit-code mapping

crates/shelfbox-core
  Public API facade, domain types, operation plans, persistence, and recovery logic
```

`crates/shelfbox` should not import `shelfbox-core` internals. Production
commands call `shelfbox_core::api` and format the returned reports for humans
or scripts.

## shelfbox

```text
src/cli.rs
  Clap command tree and top-level dispatch

src/commands/
  Command handlers, output formatting, prompts, and exit-code decisions
```

The binary crate may translate CLI flags into API calls and user-facing text.
It should not duplicate validation, ownership, recovery, or persistence rules.

## shelfbox-core Public Surface

```text
api
  Public operational entry points grouped by config, item, repo, and store;
  status APIs include legacy symlink and copy-aware v2 projections

config
  Configuration resolution and config file writes

domain
  Persistent data shapes and small invariants

error
  Shared error and result types

plan
  Dry-run and execution report shapes returned to presentation layers

prelude
  Common result/error imports for downstream callers
```

These modules are the supported crate surface. New externally callable behavior
should be exposed here deliberately, with report types that can be formatted by
the CLI without core printing user-facing text.

## shelfbox-core Private Implementation

```text
context
  Store configuration, lock acquisition, Git checkout discovery, and RepoId resolution

ops
  Operation orchestration: validate facts, call policies, execute storage/filesystem changes

policy
  Pure safety and eligibility decisions from already-collected facts

storage
  JSON persistence, atomic writes, store layout, metadata, and scanners

store
  Crate-private compatibility namespace over storage modules

fs, git, ignore, link
  Filesystem, Git, ignore-file, and symlink adapters

fs/platform
  Private no-follow, identity, link-count, atomic-replacement, and durability
  capabilities. Unsupported guarantees fail with typed errors; operations do
  not import this module.
```

Implementation modules are intentionally crate-private. Tests that need this
level of access should live inside `shelfbox-core` rather than forcing broad
public exports.

## Dependency Direction

```text
shelfbox::commands
  -> shelfbox_core::api
    -> ops
      -> policy
      -> storage/store
      -> fs/git/ignore/link
    -> domain / plan / error
```

Policy code should not perform I/O. Storage code should not own command
semantics. CLI code should not decide core safety rules. This keeps behavior
testable and keeps public API changes intentional.
