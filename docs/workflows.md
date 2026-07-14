# Common Workflows

This document describes common shelfbox tasks and recovery procedures.

For command details, see the documents under `reference/`.

---

# Keep a File Out of Git

Example:

```sh
shelfbox item add .env
```

The file is moved into the store and excluded from Git.

Typical uses:

* `.env`
* AI context files
* Local notes
* Machine-specific configuration

See:

* `reference/item-commands.md`

---

# Restore a Shelved File

Example:

```sh
shelfbox item restore .env
```

Use this when the file should become a normal repository file again.

See:

* `reference/item-commands.md`

---

# Repair a Missing Materialization

Symptoms:

```text
item status
repo status
```

reports a missing or invalid materialization.

Repair:

```sh
shelfbox item repair <PATH>
```

See:

* `reference/item-commands.md`
* `spec/failure-matrix.md`

---

# Recover After Local Index Loss

Symptoms:

```text
index.json missing or empty
repos/*/manifest.json still exists
```

Recovery:

```sh
shelfbox store rebuild-index
shelfbox repo reclaim
shelfbox repo repair
```

`repos/` is the canonical store; `index.json` is only a local cache.

If `manifest.json` itself is missing or corrupted, restore it from backup or
repair it manually before running `store rebuild-index`. shelfbox cannot infer
canonical ownership safely from loose files under `items/` alone.

See:

* `reference/repo-commands.md`
* `spec/failure-matrix.md`

---

# Recover After Repository Move

Symptoms:

```text
Repository path changed.
```

Recovery:

```sh
shelfbox repo repair
```

This refreshes local repository metadata and repairs symlinks/exclude entries
when the current clone is already associated with the existing `RepoId`.

See:

* `reference/repo-commands.md`
* `spec/ownership-model.md`

---

# Recover After Reclone

Symptoms:

```text
Repository was cloned again.
Old shelved items still exist.
```

Typical workflow:

```sh
shelfbox store rebuild-index
shelfbox repo reclaim
shelfbox repo repair
```

`item add` and `repo status` may print a reclaim hint when the new clone has no
local cache match but existing manifests match by hints. The hint is only a
guide; run `repo reclaim` to attach the clone explicitly.

See:

* `reference/repo-commands.md`
* `spec/ownership-model.md`
* `spec/failure-matrix.md`

---

# Move the Store

1. Move the store directory.
2. Update configuration.
3. Run repository repair if needed.

Example:

```sh
mv ~/.local/share/shelfbox /new/location/shelfbox
```

Then update configuration:

```sh
shelfbox config set store /new/location/shelfbox
```

See:

* `reference/config-commands.md`
* `reference/repo-commands.md`

---

# Reattach a Detached Item

A detached item is created by:

```sh
shelfbox item restore --keep-store
```

Reattach it:

```sh
shelfbox item relink <PATH>
```

See:

* `reference/item-commands.md`
* `spec/ownership-model.md`

---

# Use Copy Mode and Resolve an Edit

Enable Copy mode before creating a new item when symlinks cannot be created:

```sh
shelfbox config set materialization copy
shelfbox item add .env
```

The repository path is then an independent regular file. It remains protected
by `.git/info/exclude`, but an edit is not automatically canonical. Inspect and
choose exactly one direction:

```sh
shelfbox item status
shelfbox item sync .env --from store       # replace the repo copy
shelfbox item sync .env --from repo --yes  # replace canonical store content
```

`item repair`, `item move`, and normal `item restore` refuse to silently
overwrite a diverged copy. Synchronize first. A missing exclude entry for a
Copy item is an error; run `shelfbox repo repair` before a content mutation.

---

# PC Migration or Store Restore

When `repos/` has been restored on another machine or into a fresh store:

```sh
shelfbox store rebuild-index
shelfbox repo reclaim
shelfbox repo repair
```

This associates the current clone with the selected existing `RepoId` and then
repairs local symlinks and Git exclude entries.

See:

* `reference/repo-commands.md`
* `spec/ownership-model.md`

---

# Troubleshooting

Start with:

```sh
shelfbox repo status
```

Then consult:

* `spec/failure-matrix.md`
* `reference/item-commands.md`
* `reference/repo-commands.md`

Most recovery procedures begin with repository status and repair operations.

---

# Advanced Diagnostics

Inspect runtime context and store state:

```sh
shelfbox internal debug
```

By default, paths under your home directory are masked as `~` for safer sharing.
Use `--allow-sensitive` only when raw absolute paths are required.

Generate shell completions:

```sh
# Bash
shelfbox internal completions bash >> ~/.bash_completion

# Zsh
shelfbox internal completions zsh > ~/.zsh/completions/_shelfbox

# Fish
shelfbox internal completions fish > ~/.config/fish/completions/shelfbox.fish
```
