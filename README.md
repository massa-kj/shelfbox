# shelfbox

Keep AI context files, personal configs, and local secrets **visible in your editor** but **invisible to Git** — surviving reclones, worktrees, and index resets.

> **Linux / macOS only.** Windows support is planned for a future release.

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
- stale records left after a reclone
- store entries with no corresponding repository

Run `shelfbox repo repair` to repair the current repository's shelf, or `shelfbox repo adopt` to reclaim files after cloning a repository to a new path.

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

- **Group shelving** — shelve all files in a directory together and restore as a group: [`item add <dir>/`](docs/user-guide.md#item-add-path)
- **Recovery after reclone** — reclaim shelved items after the repository is re-cloned to a new path: [`repo adopt`](docs/user-guide.md#repo-adopt)

See [docs/user-guide.md](docs/user-guide.md) for the full command reference.

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

## Documentation

| Document | Contents |
|---|---|
| [docs/user-guide.md](docs/user-guide.md) | All commands, flags, and common workflows |
| [docs/architecture.md](docs/architecture.md) | Crate layout, data model, and design decisions |
| [docs/failure-matrix.md](docs/failure-matrix.md) | Failure modes, detection, and recovery strategies |

## License

MIT
