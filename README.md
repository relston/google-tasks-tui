# google-tasks-tui

A fast terminal client for **Google Tasks**, written in Rust
([ratatui](https://ratatui.rs) + crossterm). Each task list is a column and
its tasks run down the column. Press Enter on a task to open a details shelf
beside it.

It uses your active [Omarchy](https://omarchy.org) theme and
**live-reloads when you switch themes**. It works in any terminal: without
Omarchy it falls back to plain ANSI colors, so it picks up your terminal's
own palette.

Press `c` to create a task, `C` to create a subtask under the selected one,
`Space` to tick one off, `d` to delete it, `b` to open URLs from its notes,
`m` to move it to another list, `?` for the keyboard list, and `Tab`/`i` in
the shelf to edit its title or notes. Due dates are next. The same CRUD is
available as subcommands for scripts and agents (`google-tasks-tui lists`,
`create`, `done`, …).

## Features

- Shows all your task lists side by side. Columns scroll horizontally when
  they don't fit.
- Subtasks sit indented under their parent. Completed tasks appear struck
  through at the bottom of their list.
- Includes tasks assigned from Google Docs and Chat. Google does not allow
  those tasks to have notes or move between lists; deleting one also deletes
  its source task, so the confirmation calls that out.
- Long task titles wrap by terminal-cell width and grapheme boundaries, lined
  up under the first line rather than under the bullet. CJK, emoji, and
  combining characters keep correct cursor and column placement. Long link
  titles wrap in the Browse Link overlay too. Set `wrap_titles = false` in the
  config to clip each task to one line instead.
- Control characters in a task's text — a stray carriage return pasted into a
  title, say — are drawn as spaces instead of being sent to the terminal,
  where they would shift the rest of the row.
- Due dates are relative ("Today", "Friday", "3d overdue"). Overdue dates are
  red and today's are yellow.
- The details shelf shows status, due date, completion time, the parent task,
  subtask progress, notes, links, and the last-updated time at the bottom.
- `Space` ticks or unticks the selected task. It moves to or from the
  completed group straight away, and the selection follows it.
- `d` deletes the selected task after a y/n confirmation in the status bar.
  The prompt names the task and counts the subtasks that go with it, since
  Google deletes those too.
- `y` copies the selected task to the system clipboard as markdown, ready to
  paste into a note: the title as a heading, the due date, the notes, and the
  links Google hung off the task, such as the Gmail thread it was made from. No
  id, status or parent; those identify a task to the API and mean nothing in a
  note, and `google-tasks-tui get` is still there when you want them. No link
  back to the task itself either. The status bar confirms in green until your
  next keypress. macOS uses `pbcopy`; Linux tries `wl-copy`, `xclip`, then
  `xsel`. The OSC 52 fallback works over supported terminals and is reported as
  “sent” because terminals do not acknowledge whether they accepted it.

  ```markdown
  # Check In For Yearly Checkup

  Need my vax for newborns

  - [Check In For Your Upcoming Visit](https://mail.google.com/mail/#all/1a0…)
  ```
- `b` opens a **Browse Link** overlay of everything the selected task links
  to. Links Google attached to the task — the Gmail thread a task was made
  from, a linked Doc — are listed first under the source's own title, with
  the URL beneath it; after those come any `http://` / `https://` URLs typed
  into the notes, which have no title to show. A URL that appears both ways
  is listed once. The first link is selected; `j`/`k` move, `Enter` opens the
  selected URL and closes the overlay, `Shift+Enter` opens every URL in the
  list (selection does not matter) and closes, `Esc` closes without opening.
  If there are none, the status bar says so.
- `m` opens a **Move** overlay of every task list except the current one.
  `j`/`k` choose a list, `Enter` assigns the selected task to it (and its
  subtasks, preserving their order) and closes the overlay. Both lists are
  refreshed afterward; a partial move identifies the task that failed.
  `Esc` cancels. If there is only one list, the status bar says so.
- `?` opens a static **Keyboard** overlay listing the board commands;
  `Esc`/`q`/`?` close it.
- API calls run on a background thread, so the UI never blocks. Lists fill in
  as they load. One write runs at a time; navigation stays available while
  other mutation keys wait. If you quit while a write is pending, the app
  waits for it; press `q` or `Esc` again to force quit.
- Startup is instant: the board is painted from a local snapshot before any
  network call, then Google's copy replaces it in the background. The board is
  read-only until every list in the current sync succeeds, so you never edit
  stale data; the status bar says so while you wait.
- `c` opens a "New task" shelf next to the focused list with an empty title
  input ready to type. `Enter` creates it in that list and selects it; `Esc`
  throws the draft away. If creation fails, the same draft reopens after the
  board starts resynchronizing.
- `C` does the same but as a subtask: the shelf says "New subtask" and names
  the task it will hang under. On a top-level task that is the selection
  itself; on a subtask it is that subtask's parent, because Google Tasks only
  nests one level deep. On an empty list the status bar says there is nothing
  to put it under.
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
snapshot sits beside it in `cache.json` (also `0600`). Both files are written
atomically; the snapshot is versioned and tied to the current account so
switching credentials cannot display another account's tasks. Deleting either
file is safe: the app re-fetches, and re-authorizes. If a
google-auth `token.json` for the same client, with the Tasks scope, sits next
to `credentials.json`, the app reuses its refresh token and skips the browser
step. It only reads that file and never modifies it.

## Configuration

Settings live in `~/.config/google-tasks-tui/config.toml`, next to the token
and the board snapshot. The file is written with its defaults the first time
the app runs, so it is there to edit and documents itself. Delete a line to go
back to its default; delete the file and it comes back on the next run. A
malformed, unknown, unreadable, or unwritable setting falls back safely and is
reported in the status bar with its file path. The file is read at startup, so
changes need a restart.

| Setting       | Default | Effect                                        |
| ------------- | ------- | --------------------------------------------- |
| `wrap_titles` | `true`  | Wrap long task titles in the columns and long link titles in the Browse Link overlay onto more lines. `false` clips them to one line. |

## Install and run

Supports macOS and Linux and requires a recent stable Rust toolchain. Windows
is not currently supported because private file permissions and platform
launch/clipboard integration use Unix APIs.

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
`tasks` is `{ "lists": [ { "list": …, "tasks": […] } ] }`. `cache.json`
wraps those lists with private schema, account, and generation metadata.

`create` can also take `--parent TASK_ID`. `update` sends only the flags you
pass (`--title`, `--notes`, `--due`, `--status needsAction|completed`).
`list update` / `list delete` take a list id or unique title. `list delete`
requires `--yes` because Google removes every task in that list.

## Interface coverage

| Capability | TUI | CLI | Intent |
| ---------- | --- | --- | ------ |
| Read lists/tasks/details | Yes | Yes | Shared canonical ordering and filtering |
| Create/edit/complete/delete tasks | Yes | Yes | Same validation and API contracts |
| Create subtasks | `C` | `create --parent` | Same one-level Google constraint |
| Edit notes | Yes | Yes | Assigned Docs/Chat tasks are rejected |
| Edit due date/status directly | Status via `Space` | Yes | Due-date editor remains future TUI work |
| Move tasks between lists | Yes | No | Interactive destination picker; CLI move remains deferred |
| Task-list create/rename/delete | No | Yes | TUI list management remains deferred |
| Browse/copy task links | Yes | No | Terminal-interactive behavior |

## Keys

| Key            | Action                          |
| -------------- | ------------------------------- |
| `h` / `←`      | Previous list                   |
| `l` / `→`      | Next list                       |
| `j` / `↓`      | Next task                       |
| `k` / `↑`      | Previous task                   |
| `g` / `G`      | First / last task               |
| `c`            | Create a task in this list      |
| `C`            | Create a subtask under this one |
| `y`            | Copy the task as markdown       |
| `b`            | Open a link from the notes      |
| `m`            | Move the task to another list   |
| `Tab`          | Move between shelf fields       |
| `Space`        | Tick / untick the task          |
| `Enter`        | Open / close the details shelf  |
| `i`            | Edit the focused shelf field    |
| `d`            | Delete the task (asks y/n)      |
| `r`            | Refresh from Google             |
| `t`            | Toggle transparent background   |
| `?`            | List the keyboard commands      |
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
├── board.rs   # canonical lists/tasks, shared ordering, snapshots, rollback helpers
├── api.rs     # Google Tasks v1 client (task lists, tasks, pagination)
├── cache.rs   # board snapshot on disk for instant startup
├── config.rs  # config.toml: defaults, first-run bootstrap, parsing
├── paths.rs   # injectable credential/config paths
├── platform.rs # macOS/Linux browser and clipboard effects
├── presentation.rs # shared Markdown, validation, sanitization, URL extraction
├── storage.rs # locking and secure atomic writes
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
