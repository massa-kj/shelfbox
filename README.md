# shelfbox

Keep per-developer AI context, personal notes, and local configs **outside Git**, while keeping them visible in your editor.

Useful for AI context files, editor configs, local notes, secrets — anything that belongs in your repo tree on this machine but must never be committed.

## Typical use cases

- `CLAUDE.local.md`, `ai-context.local.md` — per-developer AI context that should not be shared with the team
- `.cursor/rules/local.mdc` — personal AI editor rules
- `.env` — local secrets and credentials
- `notes/scratch.md` — personal development notes
- `config/local.yml` — machine-specific config overrides

## How it works

```
shelfbox item add ai-context.local.md
```

1. Moves the file into a plain store directory (`~/.local/share/shelfbox/…`).
2. Leaves a symlink at the original path — your editor still sees the file.
3. Adds the path to `.git/info/exclude` so Git ignores it silently.

```
shelfbox item restore ai-context.local.md
```

Reverses the process: removes the symlink, moves the file back, and cleans the exclude entry.

The store is a regular directory of plain files, shared across all your repositories.
Each repo's files are organized under their own directory, identified by the repository root.
If anything ever goes wrong, open `~/.local/share/shelfbox/` and sort it out by hand.

## Why not `.gitignore` or `git update-index`?

`.gitignore` only affects untracked files. Files already known to Git will still appear in `git status` regardless. It also requires committing the entry, which means your teammates see it too.

`git update-index --skip-worktree` only affects the local Git index. It breaks silently across reclones, worktrees, and index resets — leaving files accidentally staged or exposed.

shelfbox physically moves the file outside the repository, so it survives:

- `git clone` / reclones
- `git worktree add`
- repository moves and renames
- index resets

Your editor still sees the file at its original path via symlink.

## Installation

### Pre-built binary (recommended)

```sh
curl -fsSL https://raw.githubusercontent.com/massa-kj/shelfbox/main/install.sh | sh
```

Installs to `~/.local/bin` by default. To specify a version or directory:

```sh
curl -fsSL https://raw.githubusercontent.com/massa-kj/shelfbox/main/install.sh | VERSION=v0.1.0 sh
curl -fsSL https://raw.githubusercontent.com/massa-kj/shelfbox/main/install.sh | INSTALL_DIR=/usr/local/bin sh
```

### From source

```sh
cargo install --path crates/shelfbox
```

Requires Rust 1.75+ and Git. Linux / macOS only (symlinks required).

## Quick start

```sh
# Shelve a file
shelfbox item add ai-context.local.md

# List shelved items
shelfbox item list

# Check health (exits 0 ok / 1 warn / 2 error)
shelfbox item status

# Restore (undo shelving)
shelfbox item restore ai-context.local.md
```

## Configuration

Optional config file at `$XDG_CONFIG_HOME/shelfbox/config.toml`
(default: `~/.config/shelfbox/config.toml`):

```toml
# Root directory for the global shelfbox store.
# Default: ~/.local/share/shelfbox
# store = "/mnt/data/shelfbox-store"

# Default output format for list/status commands.
# Valid values: table (default), plain, json
# default_format = "table"
```

The `--store <PATH>` global flag overrides config at runtime.

Inspect the current configuration:

```sh
shelfbox config list
shelfbox config explain store
```

## Non-goals

shelfbox is a **single-machine local storage** tool.

Placing the store on Dropbox, iCloud, OneDrive, or NFS may work, but is not officially supported. Sync conflicts or partial writes may leave items in an inconsistent state; run `shelfbox repo repair` to recover.

Multi-machine sync, secret encryption, and team-shared files are out of scope.

## Documentation

| Document | Contents |
|---|---|
| [docs/user-guide.md](docs/user-guide.md) | All commands, flags, and common workflows |
| [docs/architecture.md](docs/architecture.md) | Crate layout, data model, and design decisions |

## License

MIT
