# Tasks

Open work, roughly in priority order. Delete finished items and log them in
[`CHANGELOG.md`](../CHANGELOG.md). Each task keeps its number for good: new
tasks take the next unused number, and numbers are never reused.

## Improvements

- **4.** Consider checking mutation responses against the request (task ID,
  parent) as the pre-refactor handlers did. List reloads cover moves; a
  mismatched patch or create response would add a stray row.

## Features

- **11.** OAuth consent screen and Desktop client (Phase B) for project
  `gtasks-tui-pub` under publisher account `ryan@ryanelston.com` (display
  name `google-tasks-tui`, Tasks API already enabled). In Cloud Console:
  External audience in Testing, app name / support email
  `ryan@ryanelston.com`, Tasks scope
  (`https://www.googleapis.com/auth/tasks`), test users while unverified.
  Create an Auth Platform **Desktop app** OAuth client named like
  `google-tasks-tui` and download the client JSON once (the secret is only
  shown at create time) to
  `~/.config/google-tasks-tui/publisher/credentials.json` — not the git
  tree — until Phase C chooses how to ship it. Note: `gcloud iam
  oauth-clients` is for Cloud sign-in scopes, not this Auth Platform Desktop
  client — do not use it.

- **12.** Bake a default Desktop OAuth client into the app (Phase C). Resolve
  credentials as env/`GOOGLE_TASKS_CREDENTIALS` → `~/.gcloud/credentials.json`
  → built-in default. Keep the friendly missing/malformed setup guide as the
  fallback when nothing is present. Ship `client_id` + secret as approved
  defaults (Desktop secrets are public-by-design); do not commit a live JSON
  dump without an explicit choice. Update README first-run docs accordingly.
- **13.** Optional: publish the consent screen / submit sensitive-scope
  verification (Phase D) so non–test-users can sign in without the unverified
  app warning. Privacy policy URL and branding as Google requires.

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
