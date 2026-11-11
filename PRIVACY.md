# Privacy Policy

**Last updated:** 10 November 2026

This policy describes how **google-tasks-tui** (“the App”) handles information
when you use the terminal client or its CLI.

The App is open-source software published by Ryan Elston
(`ryan@ryanelston.com`). It runs on **your** computer. There is no
google-tasks-tui backend that receives your Google Tasks data.

## What the App accesses

When you sign in with Google, the App requests the OAuth scope
`https://www.googleapis.com/auth/tasks`. That lets it create, read, update,
organize, and delete **your** Google Tasks lists and tasks on your behalf
through Google’s Tasks API.

The App does not request access to Gmail, Drive, Calendar, or other Google
products beyond Tasks (except insofar as Google surfaces assigned Docs/Chat
tasks inside the Tasks API).

## What is stored on your device

After you authorize access, the App stores on your machine (under
`~/.config/google-tasks-tui/` by default):

- **OAuth tokens** (`token.json`, mode `0600`) so later runs can refresh access
  without opening a browser every time
- An optional **board cache** (`cache.json`, mode `0600`) for a faster startup
  preview
- Local **settings** (`config.toml`) such as keybindings and theme overrides

These files never leave your computer except when the App talks directly to
Google’s API endpoints using your token.

You can remove the saved sign-in with `google-tasks-tui logout` (or by deleting
`token.json`). Deleting the cache only forces a fresh download from Google.

## What is sent to Google

API requests go to Google (for example `tasks.googleapis.com` and Google’s
OAuth token endpoints). The App sends the minimum needed to perform the action
you asked for (list tasks, patch a title, move a task, and so on).

The App does **not** sell your data, run analytics on your tasks, or upload
task content to any third-party service operated by the publisher.

## OAuth client

By default the App uses a Desktop OAuth client registered by the publisher so
you do not need your own Google Cloud project. You may instead supply your own
Desktop client JSON (`GOOGLE_TASKS_CREDENTIALS` or
`~/.gcloud/credentials.json`). Desktop client secrets are treated as public by
Google’s OAuth model for installed apps.

## Children

The App is not directed at children under 13, and we do not knowingly collect
personal information from them.

## Changes

If this policy changes in a material way, the “Last updated” date above will
change and the new text will be published in this repository and at
<https://relston.github.io/google-tasks-tui/privacy/>.

## Contact

Questions about this policy: **ryan@ryanelston.com**
