# omarchy-tui

A fast terminal client for **Google Tasks**, written in Rust
([ratatui](https://ratatui.rs) + crossterm). Each task list is a column and
its tasks run down the column. Press Enter on a task to open a details shelf
beside it.

It uses your active [Omarchy](https://omarchy.org) theme and
**live-reloads when you switch themes**. It works in any terminal: without
Omarchy it falls back to plain ANSI colors, so it picks up your terminal's
own palette.

Right now it's read-only. Creating, editing and deleting tasks and lists is
next.

## Features

- Shows all your task lists side by side. Columns scroll horizontally when
  they don't fit.
- Subtasks sit indented under their parent. Completed tasks appear struck
  through at the bottom of their list.
- Due dates are relative ("Today", "Friday", "3d overdue"). Overdue dates are
  red and today's are yellow.
- The details shelf shows status, due date, completed/updated times, the
  parent task, subtask progress, notes, links and the web link.
- API calls run on a background thread, so the UI never blocks. Lists fill in
  as they load.
- Reads the active Omarchy palette (`colors.toml`) and maps it to semantic
  roles: background, foreground, surface, accent, muted, selection, red,
  green, yellow, blue.
- Supports both current (named keys like `accent`, `muted`, `red`) and legacy
  (`color0`–`color15`, `selection_background`) palettes. Named keys take
  priority.
- Picks up theme switches automatically. It checks every 500 ms whether the
  theme symlink or the palette file changed, and only reloads when one did.
- Transparent mode paints no background, so terminal opacity shows through.
- Never crashes when the theme is missing or broken. It falls back to ANSI
  colors instead.

## Google setup

You need an OAuth client of type **Desktop app** from a Google Cloud project
with the Google Tasks API enabled. Download its JSON to
`~/.gcloud/credentials.json`, or point `GOOGLE_TASKS_CREDENTIALS` at it.

On first run the app opens your browser to authorize access, then saves the
refresh token to `~/.config/omarchy-tui/token.json` (mode `0600`). If a
google-auth `token.json` for the same client, with the Tasks scope, sits next
to `credentials.json`, the app reuses its refresh token and skips the browser
step. It only reads that file and never modifies it.

## Install and run

Requires a recent stable Rust toolchain.

```sh
cargo run --release
```

The release profile uses LTO and strips the binary. To install it on your
`PATH`:

```sh
cargo install --path .
```

## Keys

| Key            | Action                          |
| -------------- | ------------------------------- |
| `h` / `←`      | Previous list                   |
| `l` / `→`      | Next list                       |
| `j` / `↓`      | Next task                       |
| `k` / `↑`      | Previous task                   |
| `g` / `G`      | First / last task               |
| `Enter`        | Open / close the details shelf  |
| `r`            | Refresh from Google             |
| `t`            | Toggle transparent background   |
| `Esc`          | Close the shelf, or quit        |
| `q`            | Quit                            |

## How theming works

The app looks for the active theme in this order:

1. `$XDG_STATE_HOME/omarchy/current` (default `~/.local/state/omarchy/current`),
   used by current Omarchy releases
2. `~/.config/omarchy/current` (legacy)

It then reads `<current>/theme/colors.toml`, a flat file of
`key = "#rrggbb"` entries. Each role uses the first key it finds:

| Role        | Keys tried (in order)                                        | Fallback        |
| ----------- | ------------------------------------------------------------ | --------------- |
| `bg`        | `background`, `color0`                                       | terminal default |
| `fg`        | `foreground`, `color7`                                       | terminal default |
| `surface`   | `lighter_background`, `color0`                               | `bg`            |
| `accent`    | `accent`, `color4`                                           | ANSI blue       |
| `muted`     | `muted`, `dark_foreground`, `color8`                         | ANSI dark gray  |
| `sel_bg`    | `selection`, `selection_background`                          | ANSI blue       |
| `sel_fg`    | `selection_foreground`, `bright_foreground`, `foreground`    | `fg`            |
| `red`       | `red`, `color1`                                              | ANSI red        |
| `green`     | `green`, `color2`                                            | ANSI green      |
| `yellow`    | `yellow`, `color3`                                           | ANSI yellow     |
| `blue`      | `blue`, `color4`                                             | ANSI blue       |

The theme name shown in the header comes from the symlink target
(`.../themes/<name>`).

## Project layout

```
src/
├── main.rs    # board UI: columns, details shelf, event loop, API worker thread
├── api.rs     # Google Tasks v1 client (task lists, tasks, pagination)
├── auth.rs    # OAuth: client secret, token cache, refresh, loopback sign-in
└── theme.rs   # theme discovery, colors.toml parsing, role mapping, fingerprint
```

## Development

```sh
cargo test
```

The test in `theme.rs` builds a fake Omarchy state directory under the system
temp dir and points `HOME` / `XDG_STATE_HOME` at it, so it doesn't touch your
real theme.

## Roadmap

- Create, edit, complete and delete tasks and task lists.
- A `theme-set.d` hook (for example through an `--install-hook` flag) that
  signals running instances for instant reload, with polling kept as a
  fallback.
- Light-mode tweaks when the theme sets `mode = "light"`.
- Move to `toml` 1.x and the latest ratatui if MSRV allows.
