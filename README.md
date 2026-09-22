# google-tasks-tui

A fast terminal client for **Google Tasks**, written in Rust
([ratatui](https://ratatui.rs) + crossterm). Each task list is a column and
its tasks run down the column. Press Enter on a task to open a details shelf
beside it.

It uses your active [Omarchy](https://omarchy.org) theme and
**live-reloads when you switch themes**. It works in any terminal: without
Omarchy it falls back to plain ANSI colors, so it picks up your terminal's
own palette.

Press `c` to create a task, `Space` to tick one off, `d` to delete it,
`b` to open URLs from its notes, and `Tab`/`i` in the shelf to edit its
title or notes. Due dates are next. The same CRUD is available as
subcommands for scripts and agents (`google-tasks-tui lists`, `create`,
`done`, …).

## Features

- Shows all your task lists side by side. Columns scroll horizontally when
  they don't fit.
- Subtasks sit indented under their parent. Completed tasks appear struck
  through at the bottom of their list.
- Due dates are relative ("Today", "Friday", "3d overdue"). Overdue dates are
  red and today's are yellow.
- The details shelf shows status, due date, completion time, the parent task,
  subtask progress, notes, links, and the last-updated time at the bottom.
- `Space` ticks or unticks the selected task. It moves to or from the
  completed group straight away, and the selection follows it.
- `d` deletes the selected task after a y/n confirmation in the status bar.
  The prompt names the task and counts the subtasks that go with it, since
  Google deletes those too.
- `b` opens a **Browse Link** overlay listing `http://` / `https://` URLs
  found in the selected task's notes. The first link is selected; `j`/`k`
  move, `Enter` opens the selected URL and closes the overlay, `Shift+Enter`
  opens every URL in the list (selection does not matter) and closes,
  `Esc` closes without opening. If there are none, the status bar says so.
- API calls run on a background thread, so the UI never blocks. Lists fill in
  as they load.
- Startup is instant: the board is painted from a local snapshot before any
  network call, then Google's copy replaces it in the background. The board is
  read-only until that first sync lands, so you never edit stale data; the
  status bar says so while you wait.
- `c` opens a "New task" shelf next to the focused list with an empty title
  input ready to type. `Enter` creates it in that list and selects it; `Esc`
  throws the draft away.
- In the shelf, `Tab` moves between the title and the notes; the focused one
  carries a marker in the accent color. `i` turns it into a text input
  (opening the shelf first if needed) and `Enter` saves to Google, `Esc`
  discards. Notes are multi-line: `Shift+Enter` starts a new line, and the
  arrows move around it.
- Editing supports `Ctrl-W` (delete word), `Ctrl-U` (clear to line start) and
  `Ctrl-A`/`Ctrl-E`. Changes show immediately; a failed save reports in the
  status bar and resyncs.
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
refresh token to `~/.config/google-tasks-tui/token.json` (mode `0600`). Later
runs reuse it without contacting Google before the first frame, and the board
snapshot sits beside it in `cache.json` (also `0600`). Deleting either file is
safe: the app re-fetches, and re-authorizes. If a
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

This puts a `google-tasks-tui` binary in `~/.cargo/bin`. Then run:

```sh
google-tasks-tui
```

With no arguments that opens the board. Any subcommand is a one-shot CLI
that uses the same OAuth token and writes `cache.json` after mutations, so
the next TUI start is current. A TUI that is already running will not pick
up those edits until you press `r`.

## CLI

Stdout is Markdown by default. Pass `-f json` / `--format json` for one JSON
value plus a newline (agents that want to parse). Errors go to stderr. Exit
`0` on success, `1` on API/auth failure, `2` on usage errors (missing
`--yes`, ambiguous list title, unknown id). `--list` accepts a list id or a
unique title. `-f` is global, so it works before or after the subcommand.

```sh
google-tasks-tui lists
google-tasks-tui -f json lists
google-tasks-tui tasks --list Inbox
google-tasks-tui get TASK_ID --list Inbox
google-tasks-tui create --list Inbox --title "Ship CLI" --notes "Phase 3" --due 2026-09-21
google-tasks-tui update TASK_ID --title "Shipped"
google-tasks-tui done TASK_ID
google-tasks-tui undone TASK_ID --list Inbox
google-tasks-tui delete TASK_ID --yes
google-tasks-tui list create --title "Work"
google-tasks-tui list update Work --title "Projects"
google-tasks-tui list delete Projects --yes
```

`lists` is a bullet list of titles with ids in backticks. `tasks` is a
heading per list and GitHub-style checkboxes (`- [ ]` / `- [x]`), with notes
indented under the item when present. `get` /
`create` / `update` / `done` / `undone` print one task as a heading plus
fields. `list create` / `list update` print one list the same way as
`lists`. `delete --yes` and `list delete --yes` print nothing. JSON shapes:
`lists` is an array of `{id, title}`; `tasks --list` is an array of tasks;
`tasks` is `{ "lists": [ { "list": …, "tasks": […] } ] }` (same as
`cache.json`).

`create` can also take `--parent TASK_ID`. `update` sends only the flags you
pass (`--title`, `--notes`, `--due`, `--status needsAction|completed`).
`list update` / `list delete` take a list id or unique title. `list delete`
requires `--yes` because Google removes every task in that list.

## Keys

| Key            | Action                          |
| -------------- | ------------------------------- |
| `h` / `←`      | Previous list                   |
| `l` / `→`      | Next list                       |
| `j` / `↓`      | Next task                       |
| `k` / `↑`      | Previous task                   |
| `g` / `G`      | First / last task               |
| `c`            | Create a task in this list      |
| `Tab`          | Move between shelf fields       |
| `Space`        | Tick / untick the task          |
| `Enter`        | Open / close the details shelf  |
| `i`            | Edit the focused shelf field    |
| `d`            | Delete the task (asks y/n)      |
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
├── main.rs    # dispatches to the TUI or the CLI
├── lib.rs     # crate root: no args → TUI, subcommand → CLI
├── tui.rs     # board UI: columns, details shelf, event loop, API worker thread
├── cli.rs     # clap subcommands; -f json|md (default md); resolve list by id or title
├── api.rs     # Google Tasks v1 client (task lists, tasks, pagination)
├── cache.rs   # board snapshot on disk for instant startup
├── auth.rs    # OAuth: client secret, token cache, refresh, loopback sign-in
└── theme.rs   # theme discovery, colors.toml parsing, role mapping, fingerprint
```

## Development

```sh
cargo test
```

`Shift+Enter` needs a terminal that speaks the kitty keyboard protocol
(Ghostty, Kitty, Alacritty, foot, WezTerm). The app asks for it at startup and
falls back to `Alt+Enter` / `Ctrl-J` elsewhere — inside tmux, that fallback is
what you'll use unless `extended-keys` is on.

The test in `theme.rs` builds a fake Omarchy state directory under the system
temp dir and points `HOME` / `XDG_STATE_HOME` at it, so it doesn't touch your
real theme.

## Roadmap

- Edit due dates; manage task lists in the TUI; move tasks between lists.
- A `theme-set.d` hook (for example through an `--install-hook` flag) that
  signals running instances for instant reload, with polling kept as a
  fallback.
- Light-mode tweaks when the theme sets `mode = "light"`.
- Move to `toml` 1.x and the latest ratatui if MSRV allows.
