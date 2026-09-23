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
  `undone`, `delete`, and `list create|update|delete`. Resolves `--list` /
  list specs by id first, then unique title. `-f` / `--format` is `md`
  (default) or `json`. Mutations go through `TasksApi`, then refresh
  `cache.json`. A
  running TUI does not live-reload CLI edits (press `r`).
- `src/presentation.rs` — shared title normalization, terminal-control
  sanitization, context-aware Markdown output (`md_task` records and
  pasteable `md_yank` notes), and validated HTTP(S) extraction that preserves
  balanced delimiters.
- `src/board.rs` — canonical task-list/task state keyed by Google IDs. It
  filters deleted tasks, derives the shared open/completed/subtask row order
  used by TUI and CLI Markdown, converts to/from cache snapshots, and owns
  task-tree move/remove/restore operations.
- `src/theme.rs` — finds the active Omarchy theme, parses `colors.toml`, maps it
  to semantic roles (`bg, fg, surface, accent, muted, sel_bg, sel_fg, red,
  green, yellow, blue`). Exposes `load()` and `fingerprint()`.
- All app config/state lives under `$XDG_CONFIG_HOME/google-tasks-tui/`.
- `src/paths.rs` — resolves credentials and application paths once; tests can
  inject a temporary `Paths` without mutating process-wide environment.
- `src/storage.rs` — exclusive file locking plus unique same-directory atomic
  writes with `0600`, file/directory sync, and rename.
- `src/platform.rs` — typed macOS/Linux URL opening and clipboard integration.
  Clipboard helpers are platform-specific; OSC 52 is reported as sent rather
  than confirmed.
- `src/config.rs` — user settings at `<config dir>/config.toml`. `load()`
  writes `TEMPLATE` only on `NotFound`. Parse, read, and write errors fall back
  safely but return a visible warning; unknown fields are rejected.
  `template_matches_the_defaults` prevents default drift. Read once at startup.
- `src/auth.rs` — OAuth for an installed client. Secret from
  `~/.gcloud/credentials.json` (`GOOGLE_TASKS_CREDENTIALS` overrides); refresh
  token cached in `$XDG_CONFIG_HOME/google-tasks-tui/token.json` (0600). Bootstraps
  from a sibling google-auth `token.json` if it has the tasks scope, else runs
  the loopback browser flow. `Auth::cached()` (no network) is the startup path;
  only a first run falls back to `Auth::new()`, which may open a browser and so
  runs before the TUI starts. A 400/401 on refresh returns typed
  `SignInExpired` and removes the app token, so restarting enters sign-in
  instead of retrying it forever. Token writes are atomic and errors propagate;
  the cache records and validates the OAuth client ID. `Auth::fingerprint`
  scopes the board cache without storing token material. The shared ureq agent
  bounds connect, send, response, body, and whole-call time. Authorization uses
  S256 PKCE, OS randomness, the shared platform opener, a five-minute callback
  deadline, five-second request reads, and an 8 KiB request-line cap.
- `src/cache.rs` — a versioned, account-scoped board preview. Locked
  generation checks reject stale cross-process writers; `Board::from_cache`
  paints before any network call and `Board::snapshot` is its data payload.
  `App::sync` is `Cached`, `Refreshing`, `Ready`, or `Failed`; writes require
  `Ready` after every list succeeds in the active refresh generation. Failed
  list loads and mutation recovery never save optimistic state.
  `check_synced()` is the TUI's only writer after a complete sync. The CLI
  reports if a confirmed remote mutation cannot update the local preview.
- `src/api.rs` — blocking Tasks v1 client on `ureq` 3. `CreateTask`,
  `TaskPatch`, `MoveTask`, and `ListPatch` are the request contracts shared by
  CLI and TUI through `TasksApi`; responses retain ETags and assignment
  context. Reads paginate, include assigned tasks, retry bounded 429/5xx using
  `Retry-After`, and retry one 401 after forcing token refresh. Writes send
  `If-Match` where supported and return typed auth/conflict/rate-limit/timeout/
  HTTP errors.
- `src/tui.rs` — board UI. A worker thread owns the `Client` and talks to the
  UI over `Cmd`/`Msg` channels. Refresh messages carry a generation so old
  responses cannot complete a newer sync. The worker is generic over
  `TasksApi`. Mutations are serialized through
  one `PendingOperation`; commands and responses carry its id and context, so
  stale or mismatched responses are ignored. Navigation remains available
  while mutation keys are blocked. The event loop polls input every 100 ms,
  drains messages, and calls `check_theme()` every 500 ms. A quit request
  waits for the pending operation; a second request force-quits. Key bindings
  require exact modifiers and Ctrl-C deliberately quits. `task_item` wraps a
  title over terminal-cell widths and grapheme boundaries when
  `config.wrap_titles` is on. Everything Google sends goes through `clean()` first: task text can
  carry control characters, and printing a `\r` walks the terminal cursor to
  column 0 so every cell after it lands in the wrong place. Clipping hid them
  past a column's edge, so wrapping is what exposed it. `clean` swaps each
  ASCII control character (bar `\n`) for a space, the same byte length, which
  keeps every offset `wrap()` and `cursor_rc()` pass around valid.
  `control_characters_never_reach_the_screen` renders through a `TestBackend`
  and asserts no cell holds one. Columns are projections of the canonical
  `Board`; `board::ordered()` supplies their rows, while selection is stored
  by task ID rather than row index. `Edit` holds
  the title buffer + byte cursor; commits are optimistic (local change first,
  then `Cmd::Patch`), and a failed save sets an error and refreshes.
  Optimistic patch/delete/move contexts retain rollback data and cache writes
  require confirmation. Ticking rebuilds the row projection immediately so
  the task moves groups while selection follows its ID. Delayed
  create responses select the new task only when selection has not moved and
  no other editor is active. `App::interaction` is the single exclusive
  `Idle` / editor / delete confirmation / Browse / Move / Help state.
  Destructive keys go through `Interaction::ConfirmDelete`, never straight to the API.
  The shelf lays out explicit sub-rects (title/meta/notes/updated) using the
  same cell-width layout for rendering and vertical cursor movement. Metadata
  is capped to reserve Notes/Updated; narrow terminals show the shelf
  full-width and tiny terminals reject blind editing. `App::field` holds shelf
  focus. `EditTarget::New` swaps the shelf for a title-only input and makes
  `commit_edit` POST instead of PATCH; its full draft lives in the pending
  operation until Google confirms it and reopens unchanged after failure.
  `C` fills its `parent` from `subtask_parent`: the selected
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
  `open` / `xdg-open` for the selected URL and dismisses the overlay only when
  launch succeeds,
  Shift+Enter opens every URL in the list then dismisses, Esc/`q` close it.
  Browse, Move, and Keyboard share bounded centered rectangles and render
  `Clear` first so underlay text cannot bleed through. `m` opens a **Move**
  overlay of every list except the current one.
  Enter uses `tasks.move` for the selected task and its subtasks, chaining
  `previous` to preserve sibling order, then fetches both authoritative lists.
  Partial moves report the failed ID. Assigned Docs/Chat tasks cannot move or
  edit notes, and their delete prompt warns that the source task is also
  deleted. `?` opens a static **Keyboard** table; Esc/`q`/`?` close it.
  Startup pushes kitty keyboard flags when the terminal supports
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
