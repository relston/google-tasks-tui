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
- [`src/tui.rs`](src/tui.rs) — thin TUI module root. It authenticates, loads
  config/cache, starts the worker, polls events and messages, and checks the
  theme every 500 ms. `TerminalGuard` owns the ratatui terminal and restores
  keyboard enhancement flags, raw mode, and the alternate screen on normal
  return, errors, and panic unwinding.
- [`src/tui/app.rs`](src/tui/app.rs) — application state and transitions.
  Columns project the canonical `Board`; selection uses task IDs. One
  `Interaction` owns the active editor/modal, and one `PendingOperation`
  serializes optimistic writes with rollback context. Refresh generations
  reject stale responses, failed writes re-enter read-only recovery, delayed
  responses do not replace another editor, and graceful quit waits for an
  in-flight write unless the user explicitly force-quits.
- [`src/tui/board.rs`](src/tui/board.rs) — TUI-specific board rules: valid move
  destinations, task/subtask move groups, one-level subtask parenting, and
  ordered Google move requests.
- [`src/tui/worker.rs`](src/tui/worker.rs) — `Cmd`/`Msg` protocol and blocking
  `TasksApi` execution on the network thread. The worker refreshes both lists
  after moves and reports partial move context.
- [`src/tui/render.rs`](src/tui/render.rs) — ratatui-only layout and drawing for
  columns, details shelves, status, and overlays. Metadata is capped to reserve
  Notes/Updated; narrow terminals use a full-width shelf; modals clear their
  underlay before drawing.
- [`src/tui/text.rs`](src/tui/text.rs) — terminal-cell/grapheme wrapping,
  clipping, padding, cursor mapping, and control-character sanitization shared
  by editing and rendering. Sanitization preserves byte lengths so cursor
  offsets remain valid.

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
