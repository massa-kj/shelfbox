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

# Repair a Missing Symlink

Symptoms:

```text
item status
repo status
```

reports a missing or invalid symlink.

Repair:

```sh
shelfbox item repair <PATH>
```

See:

* `reference/item-commands.md`
* `spec/failure-matrix.md`

---

# Recover After Manifest Loss

Symptoms:

```text
manifest.json missing
items still exist in store
```

Recovery:

```sh
shelfbox repo repair
```

The manifest is reconstructed from the deterministic store layout.

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

This updates repository metadata and applies any required ownership transitions.

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
shelfbox repo repair
shelfbox repo adopt --from <OLD_REPO_ID>
```

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

# Ownership Transfer

When ownership of shelved items must move to a new repository identity:

```sh
shelfbox repo adopt --from <OLD_REPO_ID>
```

This operation is explicit and auditable.

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
