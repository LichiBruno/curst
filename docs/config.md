# Configuration, keys and themes

Settings (**Ctrl+O**) changes the common ones; all of them are in
`~/.config/curst/config.toml` (see [where things live](collections.md)).
A value that cannot be read is reported when curst starts, and its default is
used. curst keeps your comments and layout when it writes the file.

```toml
theme = "dark"            # dark, light, mono, or one of your themes
mouse = true
sidebar_width = 30        # columns, 16 to 60

[keys]
preset = "standard"       # or "vim"

[keys.bindings]
"global.send" = "ctrl+r"
"sidebar.delete" = ["d", "delete"]

[http]
timeout = 30              # seconds, 0 for none
max_body_mb = 10          # how much of a response is kept in memory

[secrets]
backend = "keychain"      # or "encrypted-file"
```

## Keys

A binding is `"<layer>.<action>"`, and one key or a list of them. Keys are
written like `ctrl+s`, `shift+tab`, `f1`, `G`, or a sequence such as `g g`.
The help (**F1**) lists every action with its layer and its keys, and the
palette (**Ctrl+P**) shows them next to each action. A binding that cannot be
read is reported and skipped; `curst --default-keys` starts with the standard
keys if a binding locks you out.

## Themes

A theme is `~/.config/curst/themes/<name>.toml`, chosen by its name:

```toml
base = "dark"             # every role not set here comes from this theme
accent = "magenta"
muted = "#7f7f7f"
selected = "black on cyan"

[methods]
get = "bold green"
post = "yellow"
```

The roles are `accent`, `border`, `muted`, `key`, `warning`, `error`,
`success`, `info` and `selected`, and each method under `[methods]`. A color is
a name such as `red` or `cyan`, `#rrggbb`, or a number from 0 to 255, with
`bold`, `dim`, `italic`, `underlined`, `reversed`, and `on <color>` for the
background.
