# omarchy-tui

A small, fast terminal UI written in Rust ([ratatui](https://ratatui.rs) +
crossterm) that uses your active [Omarchy](https://omarchy.org) theme and
**live-reloads when you switch themes**. It works in any terminal: without
Omarchy it falls back to plain ANSI colors, so it picks up your terminal's
own palette.

Right now it's a demo app: a header showing the theme name, a colored event
list, and a status bar. It's meant as a starting point for Omarchy-native TUIs.

## Features

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
| `j` / `↓`      | Move down                       |
| `k` / `↑`      | Move up                         |
| `t`            | Toggle transparent background   |
| `r`            | Force a theme reload            |
| `q` / `Esc`    | Quit                            |

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
├── main.rs    # demo app: event loop, key handling, rendering
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

- A `theme-set.d` hook (for example through an `--install-hook` flag) that
  signals running instances for instant reload, with polling kept as a
  fallback.
- Light-mode tweaks when the theme sets `mode = "light"`.
- Move to `toml` 1.x and the latest ratatui if MSRV allows.
