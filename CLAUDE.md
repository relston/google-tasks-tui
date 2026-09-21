# google-tasks-tui

A fast Rust TUI (ratatui 0.29 + crossterm) for Google Tasks: one column per
task list, tasks listed vertically, Enter opens a details shelf, Tab moves between shelf fields (title/notes), `i` edits one inline, Space
ticks a task, `d` deletes it. Fully
integrates with the Omarchy theming system and live-reloads when the user
switches themes. Goal: full CRUD on tasks and task lists.

Work directly on `main`; no feature branches.

## Layout
- `src/theme.rs` — finds the active Omarchy theme, parses `colors.toml`, maps it
  to semantic roles (`bg, fg, surface, accent, muted, sel_bg, sel_fg, red,
  green, yellow, blue`). Exposes `load()` and `fingerprint()`.
- All app config/state lives under `$XDG_CONFIG_HOME/google-tasks-tui/`.
- `src/auth.rs` — OAuth for an installed client. Secret from
  `~/.gcloud/credentials.json` (`GOOGLE_TASKS_CREDENTIALS` overrides); refresh
  token cached in `$XDG_CONFIG_HOME/google-tasks-tui/token.json` (0600). Bootstraps
  from a sibling google-auth `token.json` if it has the tasks scope, else runs
  the loopback browser flow. Runs before the TUI starts.
- `src/api.rs` — blocking Tasks v1 client on `ureq` 3; `get_all` follows
  `nextPageToken`; `patch` PATCHes arbitrary fields of one task; `delete` removes one.
- `src/main.rs` — board UI. A worker thread owns the `Client` and talks to the
  UI over `Cmd`/`Msg` channels; the event loop polls input every 100 ms,
  drains messages, and calls `check_theme()` every 500 ms. `order()` sorts
  open tasks by position, then completed tasks newest first, with subtasks
  under their parent. `Edit` holds the title buffer + byte cursor; commits are
  optimistic (local change first, then `Cmd::Patch`), and a failed save sets an
  error and refreshes. `Msg::Updated` re-runs `order()` so a ticked task moves
  to the completed group with the selection following it. Destructive keys go
  through `App::pending` (a y/n prompt in the status bar), never straight to
  the API. The shelf lays out explicit sub-rects (title/meta/notes/updated) so
  `wrap()` + `cursor_rc()` can place the real terminal cursor; `App::field`
  holds shelf focus. Startup pushes kitty keyboard flags when the terminal
  supports them, so Shift+Enter reaches the notes editor (Alt+Enter and Ctrl-J
  are the fallbacks).

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
