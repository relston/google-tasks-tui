# Tasks

Open work, roughly in priority order. Move finished items to
[`CHANGELOG.md`](CHANGELOG.md).

## Bugs

- [ ] **A successful cross-list move can be reported as "move failed".** In
  [`src/tui/worker.rs`](src/tui/worker.rs), `Op::MoveToList` reloads both
  lists with `client.tasks(&from)?` and `client.tasks(&to)?`. If a reload
  fails after the moves succeeded, the whole op becomes an error: the status
  says "move failed", the board briefly reverts, and any partial-move detail
  is lost. Return `Ok` with a `ReloadFailed` warning (combined with the
  partial-move warning when both apply), as `Op::Place` does.
- [ ] **Queued reorder presses can disappear without a message.**
  `send_queued_reorder` in [`src/tui/commands.rs`](src/tui/commands.rs)
  skips the follow-up move when sync is not `Ready` (for example after `r`
  mid-save, since `Action::Refresh` is unconditional in
  [`src/tui/input.rs`](src/tui/input.rs)), and a `ReloadFailed` warning
  drops the queue without mentioning it. Tell the user the queued reorder
  was dropped.

## Improvements

- [ ] Replace the key hints in the bottom status line with a single hint for
  opening the keyboard shortcuts window (for example `? help`), built from
  the resolved keymap. Keep the contextual hints for editors, pickers, and
  confirmations.
- [ ] Give muted text (hints, inactive titles, completed tasks, field labels)
  its own theme role, for example Omarchy's `light_foreground`. Omarchy's
  `muted` is a border color with 1.5–3.0 contrast on most bundled themes.
- [ ] Decide whether an unknown task `status` from Google should fail the
  whole list. `Status` deserializes strictly, while due dates and timestamps
  are deliberately lenient.
- [ ] Consider checking mutation responses against the request (task ID,
  parent) as the pre-refactor handlers did. List reloads cover moves; a
  mismatched patch or create response would add a stray row.

## Features

- [ ] Edit task due dates in the TUI.
- [ ] Manage task lists in the TUI.
- [ ] Expose task moves (reorder, reparent, cross-list) through the CLI.

## Maintenance

- [ ] Reassess `toml` and ratatui upgrades against the chosen MSRV.

## Dropped

- An Omarchy `theme-set.d` hook for faster theme reload. Omarchy swaps the
  palette before retheming other apps and runs `theme-set` hooks only after
  those finish, so the 500 ms theme poll already reloads first.
- Honoring light-mode theme metadata. Omarchy themes supply every color we
  paint, and the ANSI fallback paints no background and uses reverse video
  for selections and the header badge, so the terminal (or the Omarchy theme)
  already decides light versus dark.
