# google-tasks-tui

A fast Rust TUI (ratatui 0.29 + crossterm) for Google Tasks: one column per
task list, tasks listed vertically, Enter opens a details shelf, Tab moves
between shelf fields (title/notes), `i` edits one inline, Space ticks a task,
`c` creates one, `C` a subtask, `d` deletes it, `b` opens URLs from its notes,
`m` moves it to another list, `?` lists the keys. Fully integrates with the
Omarchy theming system and live-reloads when the user switches themes. Goal:
full CRUD on tasks and task lists.

Work directly on `main`; no feature branches.

## Layout
- `src/lib.rs` — crate root. `entry()`: no clap subcommand starts the TUI;
  otherwise `Auth::cached()` / `Auth::new()` then `cli::execute_to`. Usage
  errors exit 2, API/auth errors exit 1. Stdout is markdown by default;
  `-f` / `--format json|md` (global) selects JSON for scripts.
- `src/main.rs` — binary shim that calls `entry()`.
- `src/cli.rs` — `lists`, `tasks`, `get`, `create`, `update`, `done`,
  `undone`, `delete`, and `list create|update|delete`. Two markdown shapes for
  one task, both `pub(crate)`: `md_task` for `get` (a record, ids and all) and
  `md_yank` for the TUI's `y` (a note to paste). Resolves `--list` / list
  specs by id first, then unique title. `-f` / `--format` is `md` (default) or
  `json`. Mutations go through `TasksApi`, then refresh `cache.json`. A
  running TUI does not live-reload CLI edits (press `r`).
- `src/theme.rs` — finds the active Omarchy theme, parses `colors.toml`, maps it
  to semantic roles (`bg, fg, surface, accent, muted, sel_bg, sel_fg, red,
  green, yellow, blue`). Exposes `load()` and `fingerprint()`.
- All app config/state lives under `$XDG_CONFIG_HOME/google-tasks-tui/`.
- `src/config.rs` — user settings at `<config dir>/config.toml`. `load()`
  writes `TEMPLATE` (the commented defaults) when the file is missing, and
  falls back to `Config::default()` without touching the file when it will not
  parse. `template_matches_the_defaults` fails if `TEMPLATE` and `Config`
  drift apart. Read once at startup, so edits need a restart. Settings:
  `wrap_titles` (default true).
- `src/auth.rs` — OAuth for an installed client. Secret from
  `~/.gcloud/credentials.json` (`GOOGLE_TASKS_CREDENTIALS` overrides); refresh
  token cached in `$XDG_CONFIG_HOME/google-tasks-tui/token.json` (0600). Bootstraps
  from a sibling google-auth `token.json` if it has the tasks scope, else runs
  the loopback browser flow. `Auth::cached()` (no network) is the startup path;
  only a first run falls back to `Auth::new()`, which may open a browser and so
  runs before the TUI starts. A 400/401 on refresh reports "sign-in expired".
- `src/cache.rs` — board snapshot at `<config dir>/cache.json` (0600, written
  via temp + rename). `App::new` paints from it before any network call;
  `App::synced` gates every write until this run's first full sync lands, and
  `check_synced()` rewrites the snapshot then and after each mutation. The CLI
  rewrites the snapshot after each successful mutation.
- `src/api.rs` — blocking Tasks v1 client on `ureq` 3; `get_all` follows
  `nextPageToken`; `patch` PATCHes arbitrary fields of one task; `create` POSTs
  a title with an optional parent (TUI); `insert` accepts a JSON body and a
  `parent` query (CLI); `delete` removes one; `relocate` POSTs `tasks.move` with
  `destinationTasklist` (TUI); `create_list` / `patch_list` / `delete_list`
  mutate task lists. `TasksApi` is the trait the CLI and tests call.
- `src/tui.rs` — board UI. A worker thread owns the `Client` and talks to the
  UI over `Cmd`/`Msg` channels; the event loop polls input every 100 ms,
  drains messages, and calls `check_theme()` every 500 ms. `task_item` wraps a
  title over as many lines as it needs when `config.wrap_titles` is on,
  trimming the leading space `wrap()` leaves when a break lands on a run of
  them. Everything Google sends goes through `clean()` first: task text can
  carry control characters, and printing a `\r` walks the terminal cursor to
  column 0 so every cell after it lands in the wrong place. Clipping hid them
  past a column's edge, so wrapping is what exposed it. `clean` swaps each
  ASCII control character (bar `\n`) for a space, the same byte length, which
  keeps every offset `wrap()` and `cursor_rc()` pass around valid.
  `control_characters_never_reach_the_screen` renders through a `TestBackend`
  and asserts no cell holds one. `order()` sorts open tasks by position, then
  completed tasks newest first, with subtasks under their parent. `Edit` holds
  the title buffer + byte cursor; commits are optimistic (local change first,
  then `Cmd::Patch`), and a failed save sets an error and refreshes.
  `Msg::Updated` re-runs `order()` so a ticked task moves to the completed
  group with the selection following it. Destructive keys go through
  `App::pending` (a y/n prompt in the status bar), never straight to the API.
  The shelf lays out explicit sub-rects (title/meta/notes/updated) so `wrap()`
  + `cursor_rc()` can place the real terminal cursor; `App::field` holds shelf
  focus. `App::creating` holds a `Draft` that swaps the shelf for a title-only
  input whose `Edit` has an empty `task_id`, so `commit_edit` POSTs instead of
  PATCHing. `C` fills the draft's `parent` from `subtask_parent`: the selected
  row, or the row's own parent when it is already a subtask, since Google
  Tasks nests exactly one level deep. A row that draws at depth 0 because its
  parent is missing from the list parents the new subtask itself. `y` yanks
  the selected task with `cli::md_yank` — title, due, notes and every `links`
  entry as markdown links, and none of the id / status / parent that `md_task`
  prints for `get`, nor `web_view_link`, because those identify a task to the
  API and mean nothing in a note — then runs it through `clean()`, since a
  control character pastes as badly as it renders; `copy()` tries `wl-copy` /
  `xclip` / `xsel` / `pbcopy` in turn (each exits or detaches once stdin
  closes, so waiting on one cannot stall the UI) and falls back to OSC 52 with
  a local `base64`. `App::notice` is the green counterpart to `App::error`,
  cleared at the top of `on_key` so it lasts exactly one keypress.
  `task_links` gathers what the selected task points at — the `links` Google
  attached to it (Gmail thread, Doc), which carry the source's `description`
  as a title, then `urls_in` over the notes, deduped by URL — and `b` opens a
  centered **Browse Link** overlay showing each title with its URL beneath
  (`ellipsize` trims to the overlay width); `j`/`k` move, Enter `spawn`s
  `open` / `xdg-open` for the selected URL and dismisses the overlay,
  Shift+Enter opens every URL in the list then dismisses, Esc/`q` close it. It
  renders `Clear` first, since a `Block` restyles the cells under it but
  leaves their text (`draw_move` and `draw_help` still have that bleed-
  through). `m` opens a **Move** overlay of every list except the current one;
  Enter POSTs `tasks.move` (`Cmd::Move` / `Client::relocate`) for the selected
  task and its subtasks, then follows the selection to the dest column. `?`
  opens a **Keyboard** overlay of `help_keys()`; `j`/`k` move, Esc/`q`/`?`
  close it. Startup pushes kitty keyboard flags when the terminal supports
  them, so Shift+Enter reaches the notes editor (Alt+Enter and Ctrl-J are the
  fallbacks).

## Omarchy theming facts (verified Sept 2026)
- Active theme dir, current releases: `$XDG_STATE_HOME/omarchy/current`
  (default `~/.local/state/omarchy/current`). Legacy: `~/.config/omarchy/current`.
  Always check state dir first, fall back to config.
- Palette: `<current>/theme/colors.toml`, flat `key = "#rrggbb"`.
- Current keys are named: `mode`, `accent`, `selection`, `muted`,
  `background`, `dark_background`, `darker_background`, `lighter_background`,
  `foreground`, `dark_foreground`, `light_foreground`, `bright_foreground`,
  `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `bright_*`.
- Legacy keys: `accent, cursor, foreground, background, selection_foreground,
  selection_background, color0..color15`. Support both; named keys win.
- Theme switches fire hooks in `~/.config/omarchy/hooks/theme-set.d/*.sh`.
- Users can also add `~/.config/omarchy/themed/<file>.tpl` templates with
  `{{ accent }}`-style placeholders (plus `_strip`, `_rgb` modifiers) that
  Omarchy regenerates on every theme switch.

## Design rules
- Off-Omarchy (or unparseable file) → fall back to ANSI colors (`Color::Reset`
  etc.) so the app matches any terminal. Never crash on missing theme.
- Reload only when the fingerprint (symlink target + colors.toml mtime)
  changes; keep the idle loop cheap.
- Transparent mode paints `Color::Reset` backgrounds so terminal opacity
  shows through (`t` toggles).

## Commands
- `cargo run --release` / `cargo test`
- Test in `theme.rs` builds a fake state dir via `HOME` / `XDG_STATE_HOME`.

## Next steps
1. Add a `theme-set.d` hook script (installed by the app or a `--install-hook`
   flag) that signals running instances (e.g. SIGUSR1 or a unix socket) for
   instant reload; keep polling as fallback.
2. Honor `mode = "light"` / `light.mode` if any role needs light-mode tweaks.
3. Consider `toml` 1.x and ratatui latest if MSRV allows.
