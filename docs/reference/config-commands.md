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

Supported keys: `store`, `default_format`, `materialization`.

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
```

Supported keys:

| Key | Values |
|---|---|
| `store` | Absolute path |
| `default_format` | `table`, `plain`, `json` |
| `materialization` | `symlink` (default), `copy` |

`materialization` selects the default for future materializations. It does not
convert an existing symlink or regular copy. A Copy item is a regular file in
the repository; its canonical content remains in the store and changed content
requires an explicit `item sync` direction.

---

### `config explain <KEY>`

Shows the type, default, description, and resolution precedence for a
configuration key.

```sh
shelfbox config explain store
shelfbox config explain default_format
shelfbox config explain materialization
```
