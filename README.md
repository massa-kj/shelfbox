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

```sh
cargo install --path crates/shelfbox
```

Requires Rust 1.75+ and Git. Linux / macOS only (symlinks required).

## Quick start

```sh
# Shelve a file
shelfbox add secrets/local.env

# Preview without making changes
shelfbox add notes.md --dry-run

# List shelved items
shelfbox list

# Check health
shelfbox status
shelfbox doctor

# Fix detected issues automatically
shelfbox doctor --fix

# Recreate a broken symlink
shelfbox repair secrets/local.env

# Restore
shelfbox restore secrets/local.env
```

## Configuration

Optional config file at `$XDG_CONFIG_HOME/shelfbox/config.toml`
(default: `~/.config/shelfbox/config.toml`):

```toml
# Override the store directory (default: ~/.local/share/shelfbox)
store = "/mnt/data/shelfbox-store"
```

The `--store <PATH>` global flag overrides config at runtime.

## Documentation

| Document | Contents |
|---|---|
| [docs/user-guide.md](docs/user-guide.md) | All commands, flags, and common workflows |
| [docs/architecture.md](docs/architecture.md) | Crate layout, data model, and design decisions |

## License

MIT
