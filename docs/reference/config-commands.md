## `config` — manage configuration

### `config list`

Lists all configuration keys with their current values and origins.

```sh
shelfbox config list
shelfbox config list --format json
```

**Output (table, default):**

```
KEY             TYPE  DEFAULT                  SOURCE   CURRENT
store           path  ~/.local/share/shelfbox  default  ~/.local/share/shelfbox
default_format  enum  table                    default  table
materialization enum  symlink                  default  symlink
mutation_durability enum require               default  require
```

**Flags:**

| Flag | Description |
|---|---|
| `--format <FORMAT>` | Output format: `table` (default), `json`. |

---

### `config path`

Prints the path to the active configuration file.

```sh
shelfbox config path
# /home/user/.config/shelfbox/config.toml
```

---

### `config get <KEY>`

Prints the resolved value of a configuration key. Always returns a value
(falls back to the built-in default if not configured).

```sh
shelfbox config get store
# /home/user/.local/share/shelfbox

shelfbox config get store --source
# /home/user/.local/share/shelfbox
# source: default
```

Supported keys: `store`, `default_format`, `materialization`, `mutation_durability`.

**Flags:**

| Flag | Description |
|---|---|
| `--source` | Also print where the value comes from (`cli`, `env`, `config`, `default`). |

---

### `config set <KEY> <VALUE>`

Updates a configuration key in `config.toml` without touching other content
(comments and unknown keys are preserved). Creates the file if it does not
exist.

```sh
shelfbox config set store /mnt/external/shelfbox-store
shelfbox config set default_format json
shelfbox config set materialization copy
shelfbox config set mutation_durability best-effort
```

Supported keys:

| Key | Values |
|---|---|
| `store` | Absolute path |
| `default_format` | `table`, `plain`, `json` |
| `materialization` | `symlink` (default), `copy` |
| `mutation_durability` | `require` (default), `best-effort` |

`materialization` selects the default for future materializations. It does not
convert an existing symlink or regular copy. A Copy item is a regular file in
the repository; its canonical content remains in the store and changed content
requires an explicit `item sync` direction.

`mutation_durability` is user-local configuration, not repository metadata.
`require` retains the full directory-durability protocol and fails closed
before a shelf mutation starts when the capability is unavailable (including on
Windows). `best-effort` is an explicit reduced-guarantee opt-in: it attempts
the same syncs and continues only for the typed unavailable
directory-durability capability. I/O, permission, identity, locking, and
validation errors still fail. Each successful best-effort mutation warns on
stderr; complete recovery after power loss or forced termination is not
guaranteed. `config set` itself is administrative and remains available so a
Windows user can make this opt-in.

---

### `config explain <KEY>`

Shows the type, default, description, and resolution precedence for a
configuration key.

```sh
shelfbox config explain store
shelfbox config explain default_format
shelfbox config explain materialization
shelfbox config explain mutation_durability
```
