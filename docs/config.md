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

## The vim keys

`preset = "vim"` makes curst modal, as vim is:

- **Moving around:** `h` `j` `k` `l` move in the sidebar, between a pane's
  tabs and rows, and through the response; `gg` and `G` go to the top and
  bottom.
- **Panes:** `Ctrl+h/j/k/l` move to the pane on the left, below, above or on
  the right. Many terminals send `Ctrl+h` as Backspace, so vim's own
  `Ctrl+w h/j/k/l` work too.
- **Commands:** `:` opens the command line at the bottom, as in vim:
  `:q` closes the request tab (`:q!` without asking), `:w` saves, `:wq` or
  `:x` saves and closes, `:qa` quits (`:qa!` without asking). Esc, or
  Backspace on an empty line, closes it. The command palette stays on
  `Ctrl+P`.
- **Fields:** a field never types until you ask it to. The URL starts in
  normal mode; in the request pane, `i` or Enter opens the selected field.
  `i` `a` `I` `A` type, Esc stops typing and keeps the field in normal mode,
  and Esc again leaves it. The footer shows `NORMAL` or `INSERT`.
- **In normal mode:** `h` `l` `w` `b` `e` `0` `^` `$` move; `x` `X` `D` `C`
  `dd` `cc` `dw` `cw` `ciw` change the text; `u` undoes; `j` and `k` move
  between the body's lines, or to the row above or below.

Pop-ups, such as the palette and the prompts, type at once in both presets.

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
