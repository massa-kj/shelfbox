# repo-shelve

Shelve repo-local files **outside Git**, while keeping them visible in your editor via symlinks.

Useful for local notes, AI configs, editor configs, secrets, or any file you need in the repository tree but must never be tracked by Git.

## How it works

```
repo-shelve add .env
```

1. Moves `.env` into an external store (`~/.local/share/repo-shelve/…`).
2. Leaves a symlink at the original path — your editor still sees the file.
3. Adds the path to `.git/info/exclude` so Git ignores it silently.

```
repo-shelve restore .env
```

Reverses the process: removes the symlink, moves the file back, and cleans the exclude entry.

## Installation

```sh
cargo install --path crates/repo-shelve
```

Requires Rust 1.75+ and Git. Linux / macOS only (symlinks required).

## Quick start

```sh
# Shelve a file
repo-shelve add secrets/local.env

# Preview without making changes
repo-shelve add notes.md --dry-run

# List shelved items
repo-shelve list

# Check health
repo-shelve status
repo-shelve doctor

# Restore
repo-shelve restore secrets/local.env
```

## Configuration

Optional config file at `$XDG_CONFIG_HOME/repo-shelve/config.toml`
(default: `~/.config/repo-shelve/config.toml`):

```toml
# Override the store directory (default: ~/.local/share/repo-shelve)
store = "/mnt/data/repo-shelve-store"
```

The `--store <PATH>` global flag overrides config at runtime.

## Documentation

| Document | Contents |
|---|---|
| [docs/user-guide.md](docs/user-guide.md) | All commands, flags, and common workflows |
| [docs/architecture.md](docs/architecture.md) | Crate layout, data model, and design decisions |

## License

MIT
