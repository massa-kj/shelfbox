# Getting Started

`shelfbox` keeps local files in your repository tree without allowing Git to track them.

Typical examples:

* AI context files
* Personal notes
* Local editor rules
* Secrets and credentials
* Machine-specific configuration

The file remains visible at its original path through a symlink, while the real content is stored outside the repository.

---

## Installation

### Pre-built binary

```sh
curl -fsSL https://raw.githubusercontent.com/massa-kj/shelfbox/main/install.sh | sh
```

### From source

```sh
cargo install --path crates/shelfbox
```

Requirements:

* Git
* Rust 1.75+ (source installation)
* Linux, macOS, or Windows

On Windows, symlink creation requires Developer Mode or an elevated shell.

---

## Your First Shelved File

Create a local file:

```sh
echo "my local note" > notes.local.md
```

Shelve it:

```sh
shelfbox item add notes.local.md
# notes.local.md is now a symlink; your app still reads it normally
git status  # notes.local.md does not appear — it's in .git/info/exclude
```

What happens:

1. The file is moved into the shelfbox store.
2. A symlink is created at the original path.
3. The path is added to `.git/info/exclude`.

Your editor still sees the file normally.

Verify:

```sh
shelfbox item list
```

---

## Restoring a File

To undo shelving:

```sh
shelfbox item restore notes.local.md
```

The file is moved back into the repository and the symlink is removed.

---

## Checking Health

View all managed items:

```sh
shelfbox item list
```

Check integrity:

```sh
shelfbox item status
```

Check repository-wide health:

```sh
shelfbox repo status
```

---

## Global Options and Configuration Priority

All commands accept the global `--store <PATH>` option.

```sh
shelfbox --store /tmp/my-store item list
```

Store resolution priority (highest first):

1. `--store` CLI option
2. `SHELFBOX_STORE` environment variable
3. `store` in `config.toml`
4. Built-in default (`~/.local/share/shelfbox` on Linux)

Example with environment variable:

```sh
SHELFBOX_STORE=/work/store shelfbox item list
```

---

## Common Next Steps

* Learn common recovery and migration procedures in `workflows.md`
* Learn command details in `reference/`
* Learn ownership behavior in `spec/ownership-model.md`

---

## Concepts

### Store

The store is a directory outside your repositories where shelved files are physically stored.

On Unix, shelfbox creates the store root and repository store directories with
mode `0700` (owner-only access).

### Managed Item

A file that has been moved into the store and is represented by a symlink in the repository.

### Repository Identity

Each repository registered with shelfbox has its own logical identity. This
enables recovery after repository moves and explicit reclaim after reclones.
Matching repository names or remotes may produce reclaim hints, but they do not
automatically attach a clone to an existing identity.

See `spec/ownership-model.md` for details.
