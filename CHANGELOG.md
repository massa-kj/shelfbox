# Changelog

## v0.9.0

- Added public Copy materialization mode through `materialization = "copy"`.
- Added explicit `item sync` and directional detached-item relink workflows for regular-copy content changes.
- Added copy-aware status, repair, restore, move, repository repair, and store verification, including durable mutation recovery safeguards.
- `item status` and `repo status` JSON now use status schema version 2:
  copy items expose generic materialization fields and serialize legacy `link_exists` / `link_valid` as `null`.
- `item restore --keep-store` is documented as detach semantics: it retains the observed materialization, canonical store item, manifest entry, and exclude.
- Added local `mutation_durability = "require" | "best-effort"`, defaulting to fail-closed `require`.

## v0.8.0

- Refactored the internal architecture to ensure future maintainability, extensibility, and security.
