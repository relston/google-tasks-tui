# Changelog

Notable changes to google-tasks-tui, newest first. There are no tagged
releases yet; entries are grouped by date. Open work is tracked in
[`TASKS.md`](TASKS.md).

## 2026-10-16

### Added

- Reorder a task one place among its open siblings with `Alt+j` / `Alt+k`
  (also `Alt+Down` / `Alt+Up`). Presses made while a reorder is saving move
  the row immediately and go to Google as one follow-up move.

### Changed

- Every TUI mutation now runs through one submit/rollback lifecycle
  (`App::submit`, `Op`, `Outcome`), replacing per-operation message types.
- Same-list reparenting and reordering share `Op::Place`, which reloads the
  list after the move so Google's renumbered positions replace local guesses.
- Each key action is one line in the `actions!` table; the action enum,
  lookups, and `[keys]` defaults are generated from it.
- Rendering reads `&App` without mutating it; state fix-ups moved to
  `App::settle`.
- Modules reorganized by responsibility (`model`, `board`, `google/`,
  `keys/`, `tui/view/`), and fixed states such as task status are enums.
- Hiding completed tasks keeps a completed parent visible while it has open
  subtasks.

### Fixed

- A reordered task no longer snaps back to its old place until the next
  refresh.

## 2026-10-15

### Changed

- The ANSI fallback theme draws the selection and header badge in reverse
  video, so they stay readable on light and dark terminals.

## 2026-10-07

### Added

- Toggle completed-task visibility.

## 2026-09-30

### Added

- `[theme]` overrides in `config.toml` for every semantic color role.
- `NO_COLOR` support and 256- and 16-color fallbacks for terminals without
  truecolor.

### Changed

- On Omarchy, hue roles stay on the terminal's ANSI palette; only roles with
  no ANSI equivalent come from the Omarchy theme.

## 2026-09-29

### Added

- Configurable keyboard shortcuts through `[keys]`, with conflict
  diagnostics.

## 2026-09-24 – 2026-09-26

### Added

- Task reparenting: choose a parent or promote a subtask.

### Changed

- Upgraded to ratatui 0.30. Browse, Move, Select Parent, and Keyboard share
  one filterable picker.
- Tightened reparenting rules to match what Google permits.

## 2026-09-23

### Changed

- Reliability and architecture overhaul in ten phases: per-list sync state and
  refresh generations, serialized mutations with rollback, typed Google API
  inputs and errors, bounded HTTP with safe retries, atomic `0600` token and
  cache writes, an account-scoped versioned cache, PKCE OAuth, grapheme-aware
  layout, Markdown escaping, and required fmt/Clippy/test checks.
- Task lists keep Google's source order.

## 2026-09-18 – 2026-09-22

### Added

- Initial ratatui TUI with Google OAuth and Omarchy theming.
- Edit task titles and notes, check off, delete, and create tasks and
  subtasks.
- Startup cache, a scriptable CLI with Markdown and JSON output, a link
  browser, cross-list moves, a keyboard help window, word wrap, a config file,
  and copying a task to the clipboard.
