# Tasks

Open work, roughly in priority order. Delete finished items and log them in
[`CHANGELOG.md`](../CHANGELOG.md). Each task keeps its number for good: new
tasks take the next unused number, and numbers are never reused.

## Improvements

- **4.** Consider checking mutation responses against the request (task ID,
  parent) as the pre-refactor handlers did. List reloads cover moves; a
  mismatched patch or create response would add a stray row.

## Features

- **13.** Optional: publish the consent screen / submit sensitive-scope
  verification (Phase D) so non–test-users can sign in without the unverified
  app warning. Public URLs on `relston.github.io`:
  [homepage](https://relston.github.io/google-tasks-tui/),
  [privacy](https://relston.github.io/google-tasks-tui/privacy/),
  [terms](https://relston.github.io/google-tasks-tui/terms/). Still need
  Search Console ownership of `relston.github.io`, Auth branding (those URLs
  + authorized domain + original logo), demo video, and Console
  submit/publish.

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
