# Privacy Policy

**Last updated:** 20 November 2026

This policy describes how **google-tasks-tui** (“the App”) accesses, uses, shares, protects, retains, and deletes Google user data when you use the terminal client or its CLI.

The App is open-source software published by Ryan Elston ([ryan@ryanelston.com](mailto:ryan@ryanelston.com)). It runs entirely on **your** computer. The publisher operates no server or backend, and never receives your Google user data.

## Data Accessed

When you sign in with Google, the App requests one OAuth scope: `https://www.googleapis.com/auth/tasks`. Through the Google Tasks API it accesses the following data in **your** Google account:

- **Task lists:** list IDs, titles, and last-updated times
- **Tasks:** task IDs, titles, notes, due dates, completion status and completion time, parent task, position among siblings, last-updated time, and links Google attaches to tasks (for example Docs or Chat links)

The App does not access your name, email address, contacts, Gmail, Drive, Calendar, or any other Google data.

## Data Use

The App uses Google Tasks data **only** to provide the features you ask for:

- Display your task lists and tasks in the terminal
- Create, edit, complete, move, reorder, and delete tasks and lists when you ask it to
- Search your tasks locally and copy a task as Markdown to your clipboard when you request it
- Keep a local copy so the board appears instantly on the next start

The App does not use Google user data for advertising, analytics, profiling, credit or lending decisions, or any purpose other than these user-facing features.

## Data Sharing

The App does **not** sell, rent, or transfer Google user data to anyone. Data is exchanged only between your computer and Google’s own endpoints (`tasks.googleapis.com` and Google’s OAuth endpoints). No data is sent to the publisher or any third party. The App does not use Google user data to train AI or machine-learning models, and does not send it to any AI service.

## Data Protection

- All communication with Google uses HTTPS (TLS encryption).
- The OAuth token and the board cache are stored only on your device under `~/.config/google-tasks-tui/`, written atomically with file permissions `0600` (readable only by your user account).
- The App never stores your Google password. It uses Google’s standard OAuth consent flow with PKCE.
- There is no server, so no copy of your data exists outside your computer and Google.

## Data Retention & Deletion

- **On your device:** the App keeps `token.json` (OAuth refresh token) and `cache.json` (a copy of your lists and tasks) until you delete them. The cache is replaced with fresh data from Google on every sync, so it never accumulates history.
- **On a server:** none. The publisher retains no Google user data.
- **To delete local data:** run `google-tasks-tui logout` to remove the token, and delete `~/.config/google-tasks-tui/` to remove the cache and settings. Uninstalling the App and deleting that folder removes all data the App stored.
- **To revoke access:** remove google-tasks-tui at [myaccount.google.com/permissions](https://myaccount.google.com/permissions). The App can no longer reach your data afterwards.

Your tasks themselves remain in your Google account under Google’s own retention policies; deleting a task in the App deletes it in Google Tasks.

## Limited Use

google-tasks-tui’s use and transfer of information received from Google APIs adheres to the [Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy), including the Limited Use requirements.

## OAuth client

By default the App uses a Desktop OAuth client registered by the publisher so you do not need your own Google Cloud project. You may instead supply your own Desktop client JSON (`GOOGLE_TASKS_CREDENTIALS` or `~/.gcloud/credentials.json`).

## Children

The App is not directed at children under 13, and we do not knowingly collect personal information from them.

## Changes

If this policy changes in a material way, the “Last updated” date above will change and the new text will be published here and in the [source repository](https://github.com/relston/google-tasks-tui).

## Contact

Questions about this policy: [ryan@ryanelston.com](mailto:ryan@ryanelston.com)
