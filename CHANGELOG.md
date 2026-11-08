# Changelog

Notable changes to google-tasks-tui, newest first. There are no tagged
releases yet; entries are grouped by date. Open work is tracked in
[`docs/TASKS.md`](docs/TASKS.md).

## 2026-11-08

### Fixed

- Missing or malformed Google OAuth credentials print a setup guide (Desktop
  app client, Tasks API, download path / `GOOGLE_TASKS_CREDENTIALS`) instead of
  a bare `reading …: No such file` or JSON parse error.

## 2026-11-06

### Added

- TUI `/` opens a Search picker across every list. The filter matches title
  and notes (case-insensitive); each row shows its list as the subtitle.
  Enter focuses that list and selects the task, revealing completed tasks
  when the match would otherwise be hidden.

## 2026-11-03

### Added

- TUI `Shift+d` clears completed tasks from the focused list via
  `tasks.clear` (y/n confirm). Open children of cleared parents become
  top-level; hidden completed tasks stay out of the board after reload.

## 2026-11-02

### Fixed

- Debug date month preview keeps days in fixed weekday columns (pad short
  weeks, left-align the grid) so trailing dates like `29 30` sit under Su/Mo
  instead of centering in the week.

### Added

- TUI list management: `Shift+t` creates a list (floating title panel, same as
  rename), `Shift+x` deletes the focused list (y/n confirm), `Shift+i` renames
  via that panel (Enter saves, Esc cancels), and `Shift+h` / `Shift+l` move the
  focused list one column left or right locally (Google has no list-reorder
  API; refresh restores server order).
- TUI `d` opens a floating due-date editor for the selected task (same segment
  + calendar panel as `--debug-date-time`). Enter patches the due date; Esc
  cancels. Delete moves to `x`.
- `--debug-date-time` opens a blank TUI with a centered date picker (`YYYY MM
  DD`) and a `cal`-style month preview beneath it (month name, no year; the
  selected day is highlighted). Tab / `h`/`l` / Shift+Tab / `n` / `p` /
  digits edit segments (`q` / Esc to quit).
- CLI `move` exposes the same reorder, reparent, and cross-list moves as the
  TUI: `--up` / `--down` / `--first` / `--after`, `--parent` / `--top-level`,
  and `--to-list`. Partial cross-list moves exit 1 with a count; shared board
  checks and `MoveTask::tree` keep both interfaces on one set of rules.

### Changed

- Due-date editor status hints mention `n`/`p` to adjust the focused segment
  and `h`/`l` (with Tab) to move between year, month, and day.
- `--debug-date-time` draws the segment editor and month calendar inside one
  shared bordered panel instead of two stacked boxes.
- Keep unknown Google task `status` values strict: deserialization fails the
  list rather than accepting them the way due dates and timestamps do.

## 2026-11-01

### Fixed

- The TUI enables bracketed paste with the other terminal flags, so a paste
  arrives as one `Event::Paste` instead of keypresses. Pasted text goes into
  the active editor (newlines kept in notes, flattened to spaces in titles)
  and never runs as board commands.
- After a successful CLI create, update, done, undone, delete, or list
  mutation, a failed list reload or cache write no longer exits 1. The
  command prints the result first, warns on stderr, and exits 0 so scripts
  do not retry and duplicate a create.

## 2026-10-28

### Added

- A `subtle` theme role for secondary text: hints, completed tasks, inactive
  list titles, and field labels. It defaults to the foreground faded toward
  the background at 4.5:1 contrast, so this text stays readable on every
  bundled Omarchy theme. `muted` now colors only lines and borders.

### Changed

- The Keyboard window is wider and shorter: shortcuts flow into up to three
  columns, each sized to its own keys, so descriptions are no longer cut
  off.
- The board dims behind every overlay (Keyboard, Browse, Move, Select
  Parent).
- The board's status bar shows a single `? keyboard shortcuts` hint instead
  of every key. Editors, pickers, and filters keep their contextual hints.

### Fixed

- A cross-list move whose tasks moved but whose list reload failed is no
  longer reported as "move failed" and rolled back. It reports "saved;
  reloading the list failed" and keeps the partial-move detail. A move where
  nothing moved still fails and restores the board.
- Reorder presses queued during a save no longer disappear silently: a
  refresh mid-save or a failed reload now says "queued reorder dropped".

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
