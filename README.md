# shelfbox

Keep AI context files, personal configs, and local secrets **visible in your editor** but **invisible to Git** — surviving reclones, worktrees, and index resets.

> Supported on **Linux**, **macOS**, and **Windows**. On Windows, symlink creation requires Developer Mode or an elevated shell.

## The problem

Some files need to live in your repo tree so your editor and tools can find them — but they must never be committed:

```
CLAUDE.local.md          # personal AI assistant instructions
.env                     # local secrets and credentials
config/local.yml         # machine-specific config overrides
```

**The usual approaches silently fail:**

- **`.gitignore`** — only works for files Git has never seen. Once a file is tracked, adding it to `.gitignore` does nothing. And `.gitignore` itself gets committed, so your teammates see your personal entries.
- **`git update-index --skip-worktree`** — breaks silently after `git clone`, `git worktree add`, or any index reset. The flag disappears without warning, and the file reappears as staged or modified.

**Real accidents:**

- You ran `git add .` before adding `.env` to `.gitignore`. Now it's in your commit history.
- Your personal `AGENTS.md` ended up visible in a PR diff.
- You recloned after a disk issue and lost all your AI context files.

## How it works

```sh
shelfbox item add CLAUDE.local.md
```

1. Moves the file into a plain store directory (`~/.local/share/shelfbox/`)
2. Creates a symlink at the original path — your editor and AI tools see the file as normal
3. Adds the path to `.git/info/exclude` — Git ignores it silently, nothing gets committed

```sh
shelfbox item restore CLAUDE.local.md
```

Reverses the process. The file moves back in place, the symlink is removed, and the exclude entry is cleaned up.

The store is a regular directory of plain files. It survives:

- `git clone` / reclones
- `git worktree add`
- repository moves and renames
- `git reset` and index resets

## Why not just a symlink?

Anyone can move a file and create a symlink manually. What shelfbox adds is **tracked ownership** and structured recovery. It can detect and fix:

- broken or missing symlinks
- missing `.git/info/exclude` entries
- lost local associations after a reclone
- store entries with no corresponding repository

Run `shelfbox repo repair` to repair the current repository's shelf, or `shelfbox repo reclaim` to re-associate a clone with an existing shelf after restoring `repos/`.

## Quick start

```sh
# Shelve a file
shelfbox item add CLAUDE.local.md

# List shelved items
shelfbox item list

# Check health (exits 0 ok / 1 warn / 2 error)
shelfbox item status

# Restore (undo shelving)
shelfbox item restore CLAUDE.local.md
```

## Typical use cases

| File | Why shelve it |
|---|---|
| `CLAUDE.local.md`, `AGENTS.md`, etc. | Personal AI assistant instructions |
| `.env` | Local secrets and credentials |
| `notes/scratch.md` | Personal development notes |
| `config/local.yml` | Machine-specific config overrides |

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

Requires Rust 1.75+ and Git.

## More features

- **Directory shelving** — shelve eligible files under a directory; each file remains an independent item: [`item add <PATH>`](docs/reference/item-commands.md#item-add-path)
- **Recovery after reclone** — re-associate a new clone with an existing shelf after restoring `repos/`: [`repo reclaim`](docs/reference/repo-commands.md#repo-reclaim)
- **Store recovery** — rebuild local cache files from canonical manifests: [`store rebuild-index`](docs/reference/store-commands.md#store-rebuild-index)

See [docs/index.md](docs/index.md) for the full documentation set.

## Configuration

Optional config at `~/.config/shelfbox/config.toml` (respects `$XDG_CONFIG_HOME`):

```toml
# store = "/mnt/data/shelfbox-store"   # default: ~/.local/share/shelfbox
# default_format = "table"             # table | plain | json
```

The `--store <PATH>` global flag overrides config at runtime.

```sh
shelfbox config list
shelfbox config explain store
```

## Non-goals

shelfbox is a **single-machine** tool. Placing the store on external or network-synced storage is not officially supported — sync conflicts may leave items in an inconsistent state.

Multi-machine sync, secret encryption, and team-shared files are out of scope.

## [Documentation](docs/index.md)

### New users

| Document | Description |
|---|---|
| [Getting Started](docs/getting-started.md) | Installation, basic concepts, and first-time usage |
| [Workflows](docs/workflows.md) | Common tasks and recovery procedures |

### Command Reference

| Document | Description |
|---|---|
| [Item Commands](docs/reference/item-commands.md) | Item management commands |
| [Repository Commands](docs/reference/repo-commands.md) | Repository management commands |
| [Store Commands](docs/reference/store-commands.md) | Store management commands |
| [Configuration Commands](docs/reference/config-commands.md) | Configuration commands |

### Architecture

| Document | Description |
|---|---|
| [Architecture Overview](docs/architecture/architecture-overview.md) | System architecture and component boundaries |
| [Data Model](docs/architecture/data-model.md) | Store layout, manifests, and persistent data |
| [Design Decisions](docs/architecture/design-decisions.md) | Design rationale and implementation choices |

### Specifications

| Document | Description |
|---|---|
| [Ownership Model](docs/spec/ownership-model.md) | Ownership state machine and transition rules |
| [Failure Matrix](docs/spec/failure-matrix.md) | Failure modes, recoverability, and recovery guarantees |

## License

MIT
