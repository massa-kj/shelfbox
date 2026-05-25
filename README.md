# shelfbox

Shelve repo-local files **outside Git**, while keeping them visible in your editor via symlinks.

Useful for local notes, AI configs, editor configs, secrets, or any file you need in the repository tree but must never be tracked by Git.

## How it works

```
shelfbox add .env
```

1. Moves `.env` into an external store (`~/.local/share/shelfbox/…`).
2. Leaves a symlink at the original path — your editor still sees the file.
3. Adds the path to `.git/info/exclude` so Git ignores it silently.

```
shelfbox restore .env
```

Reverses the process: removes the symlink, moves the file back, and cleans the exclude entry.

## Installation

### Pre-built binary (recommended)

```sh
curl -fsSL https://raw.githubusercontent.com/massa-kj/shelfbox/main/install.sh | sh
```

Installs to `~/.local/bin` by default. To specify a version or directory:

```sh
VERSION=v0.1.0 curl -fsSL https://raw.githubusercontent.com/massa-kj/shelfbox/main/install.sh | sh
INSTALL_DIR=/usr/local/bin curl -fsSL https://raw.githubusercontent.com/massa-kj/shelfbox/main/install.sh | sh
```

### From source

```sh
cargo install --path crates/shelfbox
```

Requires Rust 1.75+ and Git. Linux / macOS only (symlinks required).

## Quick start

```sh
# Shelve a file
shelfbox item add ai-config.local.md

# Preview without making changes
shelfbox item add .env --dry-run

# List shelved items
shelfbox item list

# Check health of shelved items
shelfbox item status

# Full integrity check of the current repo
shelfbox repo status

# Fix detected issues automatically
shelfbox repo repair

# Recreate a broken symlink
shelfbox item repair ai-config.local.md

# Restore (undo shelving)
shelfbox item restore ai-config.local.md
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

## Documentation

| Document | Contents |
|---|---|
| [docs/user-guide.md](docs/user-guide.md) | All commands, flags, and common workflows |
| [docs/architecture.md](docs/architecture.md) | Crate layout, data model, and design decisions |

## License

MIT
