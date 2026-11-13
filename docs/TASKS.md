# Tasks

Open work, roughly in priority order. Delete finished items and log them in
[`CHANGELOG.md`](../CHANGELOG.md). Each task keeps its number for good: new
tasks take the next unused number, and numbers are never reused.

## Improvements

- **4.** Consider checking mutation responses against the request (task ID,
  parent) as the pre-refactor handlers did. List reloads cover moves; a
  mismatched patch or create response would add a stray row.

## Features

- **13.** Phase D sensitive-scope verification for
  `https://www.googleapis.com/auth/tasks` on project `gtasks-tui-pub`
  (**submitted 2026-11-12**, awaiting Google). Branding is published on
  owned domain `ryanelston.com`:
  [homepage](https://ryanelston.com/google-tasks-tui/),
  [privacy](https://ryanelston.com/google-tasks-tui/privacy/),
  [terms](https://ryanelston.com/google-tasks-tui/terms/). Demo video and
  Console application are in. Keep until consent shows Verified / no
  unverified-app warning for non–test-users.

## Maintenance

- **9.** Reassess `toml` and ratatui upgrades against the chosen MSRV.

## Dropped

- An Omarchy `theme-set.d` hook for faster theme reload. Omarchy swaps the
  palette before retheming other apps and runs `theme-set` hooks only after
  those finish, so the 500 ms theme poll already reloads first.
- Honoring light-mode theme metadata. Omarchy themes supply every color we
  paint, and the ANSI fallback paints no background and uses reverse video
  for selections and the header badge, so the terminal (or the Omarchy theme)
  already decides light versus dark.
