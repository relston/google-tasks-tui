# google-tasks-tui

A fast terminal client for **Google Tasks**, written in Rust
([ratatui](https://ratatui.rs) + crossterm). Lists are columns; tasks run
down each column. Press Enter for a details shelf. Same CRUD is available as
CLI subcommands for scripts and agents.

## Features

### Built for speed

- Instant startup from a local cache; Google refreshes in the background.
  Board stays read-only until that sync finishes — no edits against stale
  data.
- Non-blocking UI: API work is off-thread, lists stream in, writes run one at
  a time while navigation stays live. Quit waits for pending writes; `q`
  again force-quits.

### Fits your theme

- Semantic roles (`accent`, `muted`, `selection`, …) from the terminal ANSI
  palette, then [Omarchy](https://omarchy.org) when present, then `[theme]`
  overrides in `config.toml`.
- Honors `NO_COLOR`; falls back to 256- or 16-color. Live-reloads on Omarchy
  theme switch. Transparent mode shows terminal opacity. Missing or broken
  themes fall back to ANSI — never a crash.

### Full CLI

- Same OAuth token and cache as the TUI. Markdown by default, `-f json` for
  agents. Lists, tasks, create/update/done, move, list CRUD, logout — see
  [CLI](#cli).

### Quality of life

- All lists side by side; subtasks indented; `v` hides completed (completed
  parents with open children stay). Docs/Chat-assigned tasks are included,
  with Google’s edit limits called out on confirm.
- `/` searches every list (title + notes); `Enter` jumps to the match.
- `b` browses Google-attached and notes URLs; `Enter` opens one,
  `Shift+Enter` opens all matching.
- `y` yanks paste-ready markdown (title, due, notes, links):

  ```markdown
  # Check In For Yearly Checkup

  Need my vax for newborns

  - [Check In For Your Upcoming Visit](https://mail.google.com/mail/#all/1a0…)
  ```
- Relative due dates ("Today", "3d overdue"); overdue red, today yellow.
- Details shelf: status, due, notes, links, parent / subtask progress.
- Every overlay filters with `/` (case-insensitive substring on title /
  subtitle; Search also matches notes).

### Remappable keys

Defaults below; override any action under `[keys]` in `config.toml`
(`--default-config` prints the catalog; `?` shows live bindings).

- `c` / `C` — new task / new subtask
- `Space` — tick / untick (selection follows)
- `i` / `Tab` — edit shelf fields; `d` — due-date picker
- `x` / `D` / `X` — delete task / clear completed / delete list (y/n)
- `T` / `I` — create / rename list; `H`/`L` — reorder columns (local)
- `m` — move to another list (keeps subtask order)
- `<` / `>` — unparent / choose parent
- `Alt-j` / `Alt-k` — reorder among siblings
- `?` — keyboard overlay

## Google setup

On first run the app uses a **shipped Desktop OAuth client** and opens your
browser to authorize Google Tasks access. No Google Cloud project of your own
is required. The refresh token is saved to
`~/.config/google-tasks-tui/token.json` (mode `0600`). Later runs reuse it
without contacting Google before the first frame, and the board snapshot sits
beside it in `cache.json` (also `0600`). Both files are written atomically;
the snapshot is versioned and tied to the current account so switching
credentials cannot display another account's tasks. Run `google-tasks-tui
logout` (or delete the token file) to forget the sign-in; deleting the cache
is also safe — the app re-fetches, and re-authorizes.

To bring your own OAuth client instead, download a **Desktop app** JSON from a
Google Cloud project with the Tasks API enabled and either save it as
`~/.gcloud/credentials.json` or point `GOOGLE_TASKS_CREDENTIALS` at it. A file
at that path wins over the shipped client. If `GOOGLE_TASKS_CREDENTIALS` is
set but missing or not a Desktop client JSON, the app exits with a short setup
guide instead of a bare IO error. If a google-auth `token.json` for the same
client, with the Tasks scope, sits next to your credentials file, the app
reuses its refresh token and skips the browser step. It only reads that file
and never modifies it.

While the publisher consent screen is in Testing, only listed test users can
complete Google's consent flow; strangers see the unverified-app warning until
sensitive-scope verification is finished.

Public branding / verification URLs (authorized domain `ryanelston.com`):

- Homepage: <https://ryanelston.com/google-tasks-tui/>
- [Privacy Policy](https://ryanelston.com/google-tasks-tui/privacy/)
- [Terms of Service](https://ryanelston.com/google-tasks-tui/terms/)

Canonical Markdown copies also live in this repo as [PRIVACY.md](PRIVACY.md)
and [TERMS.md](TERMS.md).

## Configuration

Settings live in `~/.config/google-tasks-tui/config.toml`, next to the token
and the board snapshot. The file is written with its defaults the first time
the app runs, so it is there to edit and documents itself. Delete a line to go
back to its default; delete the file and it comes back on the next run. A
malformed, unknown, unreadable, or unwritable setting falls back safely and is
reported in the status bar with its file path. The file is read at startup, so
changes need a restart.

`wrap_titles = true` wraps long task and link titles. Set it to `false` to
clip them to one line.

Every keyboard command can be overridden under `[keys]`. A binding can be one
string, an array of alternatives, or an empty string/array to disable the
action:

```toml
[keys]
next_task = ["j", "down"]
create_task = "n"
create_subtask = "shift+n"
help = ["shift+slash", "f1"]
delete_task = ""
```

Bindings accept plain keys, `ctrl`, `shift`, `alt`, `cmd`/`super`, function
keys, arrows, `enter`, `esc`, `tab`, and named punctuation such as `comma`,
`period`, `slash`, and `plus`. Uppercase letters imply Shift. Modifier chords
such as `ctrl+b` are matched directly.

Every action name, grouped by where it works and listed with its default,
is in the commented configuration. Print it without signing in:

```sh
google-tasks-tui --default-config
```

In the app, `?` opens the Keyboard overlay, which lists every action with the
keys it is bound to right now. On the board the status bar shows only that
key (`? keyboard shortcuts`); editors, pickers, and filters show the keys
that apply there.

The Keyboard overlay and status hints always show the active bindings.
Conflicting user bindings report which action won. User overrides displace
conflicting built-in defaults. Unmodified printable editor/filter bindings
are rejected because they would prevent typing. An unknown action name is
reported in the status bar and ignored; the rest of the file still applies.

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
google-tasks-tui move TASK_ID --up
google-tasks-tui move TASK_ID --after OTHER_ID
google-tasks-tui move TASK_ID --parent PARENT_ID
google-tasks-tui move TASK_ID --top-level
google-tasks-tui move TASK_ID --to-list Work
google-tasks-tui list create --title "Work"
google-tasks-tui list update Work --title "Projects"
google-tasks-tui list delete Projects --yes
google-tasks-tui logout
```

`logout` deletes `~/.config/google-tasks-tui/token.json` without contacting
Google. The next TUI or CLI run signs in again (browser consent). It does not
remove a BYO `~/.gcloud/credentials.json`; if a google-auth `token.json`
beside that file still matches, that refresh token may still be reused.

`lists` is a bullet list of titles with ids in backticks. `tasks` is a
heading per list and GitHub-style checkboxes (`- [ ]` / `- [x]`), with notes
indented under the item when present. `get` /
`create` / `update` / `done` / `undone` / `move` print one task as a heading
plus fields. `list create` / `list update` print one list the same way as
`lists`. `delete --yes` and `list delete --yes` print nothing. JSON shapes:
`lists` is an array of `{id, title}`; `tasks --list` is an array of tasks;
`tasks` is `{ "lists": [ { "list": …, "tasks": […] } ] }`. `cache.json`
wraps those lists with private schema, account, and generation metadata.

`create` can also take `--parent TASK_ID`. `update` sends only the flags you
pass (`--title`, `--notes`, `--due`, `--status needsAction|completed`).
`move` takes exactly one of `--up`, `--down`, `--first`, `--after ID`,
`--parent ID`, `--top-level`, or `--to-list LIST` (same rules as the TUI).
`list update` / `list delete` take a list id or unique title. `list delete`
requires `--yes` because Google removes every task in that list.

## Interface coverage

| Capability | TUI | CLI | Intent |
| ---------- | --- | --- | ------ |
| Read lists/tasks/details | Yes | Yes | Shared canonical ordering and filtering |
| Create/edit/complete/delete tasks | Yes | Yes | Same validation and API contracts |
| Create subtasks | `C` | `create --parent` | Same one-level Google constraint |
| Reparent/unparent tasks | `<` / `>` | `move --parent` / `--top-level` | Same-list parent rules |
| Reorder tasks in a list | `Alt-j` / `Alt-k` | `move --up` / `--down` / `--first` / `--after` | One place among open siblings |
| Edit notes | Yes | Yes | Assigned Docs/Chat tasks are rejected |
| Edit due date/status directly | Due via `d`; status via `Space` | Yes | Same `tasks.patch` path |
| Move tasks between lists | Yes | `move --to-list` | Task plus subtasks; partial moves report a count |
| Task-list create/rename/delete | Create `T`, rename `I`, delete `X` | Yes | Column reorder `H`/`L` is local only |
| Search tasks across lists | `/` | No | Title and notes; Enter focuses the match |
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
| `/`            | Search tasks across lists       |
| `m`            | Move the task to another list   |
| `Alt-j` / `Alt-k` | Move the task down / up one place |
| `<` / `>`      | Unparent / choose a parent      |
| `Tab`          | Move between shelf fields       |
| `Space`        | Tick / untick the task          |
| `v`            | Show / hide completed tasks     |
| `Enter`        | Open / close the details shelf  |
| `i`            | Edit the focused shelf field    |
| `d`            | Edit the task's due date         |
| `x`            | Delete the task (asks y/n)      |
| `X`            | Delete the focused list (asks y/n) |
| `I`            | Rename the focused list         |
| `H` / `L`      | Move list left / right (local)  |
| `r`            | Refresh from Google             |
| `t`            | Toggle transparent background   |
| `?`            | List the keyboard commands      |
| `/`            | Search (board) / filter overlay |
| `Esc`          | Close or cancel the current view |
| `q`            | Quit                            |

## How theming works

Colors resolve in three layers, each overriding the one before:

1. **Terminal ANSI palette.** Every role defaults to an ANSI color or the
   terminal's default background and foreground, so the app matches
   whatever your terminal is themed with. It paints no background of its
   own, and selections and the header badge use reverse video, so it reads
   correctly whether the terminal is light or dark. There is no separate
   light/dark setting to detect: the terminal (or the Omarchy theme) is that
   signal.
2. **Omarchy**, when `source = "auto"` (the default) finds an active theme.
3. **`[theme]` in `config.toml`.**

```toml
[theme]
source = "auto"          # or "ansi" to ignore Omarchy
accent = "magenta"       # ANSI name: black, red, …, white, bright-<name>
muted = 244              # 0-255 palette index
selection = "#3b4252"    # 24-bit
background = "reset"     # the terminal's default
```

Roles: `background`, `foreground`, `surface`, `accent`, `muted`, `subtle`,
`selection`, `selection_foreground`, `red`, `green`, `yellow`, `blue`. A
`selection` of `"reset"` means reverse video. `muted` colors lines such as
inactive column borders; `subtle` colors text that should recede (hints,
completed tasks, inactive list titles, field labels). When the final
foreground and background are both 24-bit, `subtle` defaults to the
foreground faded toward the background until it reaches 4.5:1 contrast;
otherwise it is ANSI bright black. An
invalid color is reported in the status bar and the whole config falls back
to its defaults.

Finally, colors are fitted to the terminal:

- A non-empty `NO_COLOR` turns color off. Selections switch to reverse video
  so they stay visible.
- 24-bit colors are used when `COLORTERM` is `truecolor` or `24bit`, or
  `TERM` names a truecolor terminal (`*-direct`, kitty, ghostty, alacritty,
  foot, wezterm). Otherwise they map to the nearest 256-color entry, or to
  the nearest of the 16 ANSI colors when `TERM` has no 256-color support.

### Omarchy

The app looks for the active theme in this order:

1. `$XDG_STATE_HOME/omarchy/current` (default `~/.local/state/omarchy/current`),
   used by current Omarchy releases
2. `~/.config/omarchy/current` (legacy)

It then reads `<current>/theme/colors.toml`, a flat file of
`key = "#rrggbb"` entries. Only roles with no ANSI equivalent are read from
it. Red, green, yellow, and blue stay ANSI: Omarchy themes the terminal's
palette with the same values, so they look identical and keep working if you
run the app in a terminal Omarchy doesn't theme. Each role uses the first
key it finds:

| Role        | Keys tried (in order)                                        | Fallback        |
| ----------- | ------------------------------------------------------------ | --------------- |
| `bg`        | `background`, `color0`                                       | terminal default |
| `fg`        | `foreground`, `color7`                                       | terminal default |
| `surface`   | `lighter_background`, `color0`                               | `bg`            |
| `accent`    | `accent`, `color4`                                           | ANSI blue       |
| `muted`     | `muted`, `dark_foreground`, `color8`                         | ANSI bright black |
| `subtle`    | derived from `fg` and `bg` (4.5:1 contrast)                  | ANSI bright black |
| `sel_bg`    | `selection`, `selection_background`                          | reverse video   |
| `sel_fg`    | `selection_foreground`, `bright_foreground`, `foreground`    | `fg`            |

The theme name shown in the status bar comes from the symlink target
(`.../themes/<name>`).

## Project layout

```
src/
├── main.rs         # dispatches to the TUI or the CLI
├── lib.rs          # crate root: builds paths; no args → TUI, subcommand → CLI
├── model.rs        # Task, TaskList, Status, due dates, title and URL rules
├── board.rs        # canonical lists/tasks, ordering, tree queries and reorder plans
├── google/
│   ├── mod.rs      # TasksApi trait and typed errors
│   ├── client.rs   # Google Tasks v1 client: retries, pagination, endpoints
│   ├── requests.rs # typed request bodies
│   ├── auth.rs     # OAuth: client secret, token cache, refresh, loopback sign-in
│   └── http.rs     # bounded HTTP agent and URL encoding
├── keys/
│   ├── actions.rs  # action catalog and default bindings
│   ├── combo.rs    # key parsing, matching, and labels
│   └── keymap.rs   # per-context resolution, overrides, diagnostics
├── cli.rs          # clap subcommands; -f json|md (default md); resolve list by id or title
├── markdown.rs     # Markdown output for the CLI and the copy command
├── cache.rs        # board snapshot on disk for instant startup
├── config.rs       # config.toml: defaults, first-run bootstrap, parsing
├── paths.rs        # injectable credential/config paths
├── atomic_file.rs  # locking and secure atomic writes
├── desktop.rs      # macOS/Linux browser and clipboard effects
├── theme.rs        # theme layers, Omarchy colors.toml, [theme] overrides, color support
├── tui.rs          # TUI startup, terminal guard, and event loop
└── tui/
    ├── app.rs      # App state, construction, quit, and message dispatch
    ├── input.rs    # key → action per context → handler
    ├── commands.rs # what each user command does
    ├── mutation.rs # the in-flight mutation, its reply, and rollback
    ├── sync.rs     # refresh generations, load state, recovery, cache save
    ├── columns.rs  # visible rows and selection
    ├── editor.rs   # title/notes edit buffer
    ├── picker.rs   # filterable modal list
    ├── worker.rs   # API worker protocol and command execution
    └── view/       # drawing: layout, columns, shelf, overlay, status, text
```

## Development

```sh
cargo test
google-tasks-tui --debug-date-time   # blank TUI; Tab/h/l / n/p / digits edit date
```

`Shift+Enter` needs a terminal that speaks the kitty keyboard protocol
(Ghostty, Kitty, Alacritty, foot, WezTerm). The app asks for it at startup and
falls back to `Alt+Enter` / `Ctrl-J` elsewhere — inside tmux, that fallback is
what you'll use unless `extended-keys` is on.

Tests inject temporary configuration and theme roots; they do not change
process-wide environment variables or touch your real files.

## Tasks and changes

Open work is tracked in [`docs/TASKS.md`](docs/TASKS.md); notable changes are
in [`CHANGELOG.md`](CHANGELOG.md).
