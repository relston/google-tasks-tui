# Reliability and Architecture Roadmap

This roadmap prioritizes correctness and data safety before structural cleanup,
rendering polish, and new features. Complete phases in order. Write each test
first, confirm that it fails for the expected reason, then implement the fix.

## Phase 1: Close immediate correctness gaps

Prevent stale writes, lost mutations, and unrecoverable authentication before
changing the broader architecture.

- [x] a) Add state-transition tests proving that a failed task-list fetch does
  not mark the board synchronized, enable mutations, or replace the cache.
- [x] b) Replace the boolean load gate in [`src/tui.rs`](src/tui.rs) with
  explicit per-list success/failure state. Only a completely successful sync
  may make the board writable or save a snapshot.
- [x] c) Route every mutation failure through one recovery transition that
  immediately makes the board read-only. Never save failed optimistic deletes
  or moves.
- [x] d) Add refresh generation IDs to `Cmd` and `Msg`; ignore responses from
  an older refresh.
- [x] e) Preserve the current selection when delayed create/update responses
  arrive. A response must never hide an editor targeting another task.
- [x] f) Make shutdown wait for queued and in-flight writes, with an explicit
  force-quit path rather than silently terminating the worker.
- [x] g) Introduce a typed expired-authentication result. Invalidate a rejected
  refresh token so restarting reaches authorization instead of reloading the
  same unusable token.
- [x] h) Configure bounded HTTP connect, response, body, and global timeouts so
  the CLI and worker cannot wait indefinitely.

## Phase 2: Make application state explicit

Remove the combinations of booleans and optional values that currently encode
exclusive UI and synchronization states.

- [x] a) Replace `edit`, `creating`, `pending`, `browse`, `move_to`, and `help`
  with one `Interaction` enum.
- [x] b) Replace the empty-task-ID creation sentinel with an `EditTarget` enum
  for existing tasks and new tasks.
- [x] c) Replace `fetching` and `synced` with a `SyncState` that distinguishes
  cached, refreshing, ready, and failed states.
- [x] d) Permit navigation while saving but serialize mutations to one
  in-flight write. Block conflicting mutation keys until it resolves.
- [x] e) Carry operation context through worker responses so success, failure,
  rollback, selection, and status messages apply to the correct task.
- [x] f) Retain new-task drafts until Google confirms creation; reopen the
  editor with the original text after failure.

## Phase 3: Separate canonical board data from rendering

Stop using rendered rows as canonical, optimistic, selected, and persisted
state simultaneously.

- [x] a) Introduce a `Board` model containing canonical task lists and tasks,
  keyed by stable IDs.
- [x] b) Store selection by task ID rather than rendered row index.
- [x] c) Derive sorted and indented rows from canonical tasks through one
  shared ordering function.
- [x] d) Use the same deleted-task filtering, ordering, and subtask semantics
  in the TUI, CLI Markdown, and cache conversion.
- [x] e) Apply optimistic changes as an explicit pending projection or retain
  rollback data. Persist confirmed state only.
- [x] f) Reorder completion changes immediately while preserving selection,
  then replace temporary values with Google’s response.

## Phase 4: Unify and strengthen the Google API boundary

Make the existing API seam represent every operation used by both interfaces.

- [x] a) Replace raw JSON request arguments with typed `CreateTask`,
  `TaskPatch`, and `MoveTask` inputs in [`src/api.rs`](src/api.rs).
- [x] b) Add move support to `TasksApi` and make both the CLI and TUI worker
  depend on the same interface and test double.
- [x] c) Preserve subtask order during cross-list moves by sending the
  `previous` sibling parameter documented by Google.
- [x] d) Return authoritative moved tasks, including their new parent,
  position, and timestamps. Refresh both affected lists before confirming a
  multi-task move.
- [x] e) Report partial moves with the IDs that succeeded and the ID that
  failed. Investigate safe compensation without claiming the operation is
  atomic.
- [x] f) Add `showAssigned=true` when listing tasks, or explicitly document
  that assigned Docs/Chat tasks are excluded. If included, model and enforce
  their mutation restrictions.
- [x] g) Parse Google errors into typed authentication, conflict, rate-limit,
  timeout, and general HTTP failures.
- [x] h) Retry one request after refreshing an unexpectedly rejected access
  token. Add bounded retries only for safe reads and honor `Retry-After`.
- [x] i) Retain task and task-list ETags and verify conditional-write support
  against the Tasks API before using them to prevent external-edit conflicts.

## Phase 5: Make persistence secure and predictable

Give configuration, token, and snapshot storage a single owner rather than
routing application paths through the authentication module.

- [x] a) Move application-directory discovery out of
  [`src/auth.rs`](src/auth.rs) into a dedicated paths/storage module that can
  be injected in tests.
- [x] b) Implement one secure atomic-write helper using unique same-directory
  temporary files, restrictive permissions, flush/sync, and atomic rename.
- [x] c) Use atomic writes for `token.json`; return errors instead of silently
  truncating or failing to save the token.
- [x] d) Make cache writes safe across CLI and TUI processes. Prevent a stale
  process from replacing a newer snapshot.
- [x] e) Define the cache as a disposable, read-only startup preview and write
  it only from confirmed state after a complete successful sync.
- [x] f) Add a cache schema version and token/account fingerprint so switching
  credentials cannot display another account’s cached tasks.
- [x] g) Store the OAuth client ID with the token and reject cached tokens
  created for a different client.
- [x] h) Return visible warnings for cache/config persistence failures. A
  successful remote mutation must not silently claim that local cache repair
  succeeded.
- [x] i) Parse configuration with unknown-field detection. Bootstrap only on
  `NotFound`; report malformed, unreadable, or unwritable files with their
  path and parse location.

## Phase 6: Harden OAuth and platform effects

Centralize OS integration and make failures visible.

- [x] a) Extract browser and clipboard operations from
  [`src/tui.rs`](src/tui.rs) into a platform module returning typed results.
- [x] b) Reuse the platform browser opener for OAuth so macOS uses `open` and
  Linux uses `xdg-open`.
- [x] c) Add S256 PKCE to the installed-app OAuth flow.
- [x] d) Add bounded reads and timeouts to the loopback callback listener.
- [x] e) Replace `/dev/urandom` with an OS-backed random source.
- [x] f) Select clipboard helpers by platform. Distinguish a confirmed helper
  copy from an unacknowledged OSC 52 sequence in the status message.
- [x] g) Keep the Browse overlay open and show an error when opening a URL
  fails.
- [x] h) Either conditionally support non-Unix platforms or state the
  supported operating systems explicitly.

## Phase 7: Correct terminal layout and rendering

Make text measurement and modal behavior consistent across terminal sizes and
Unicode input.

- [x] a) Use terminal-cell width and grapheme-aware cursor movement for edit
  views, wrapping, clipping, padding, and cursor placement.
- [x] b) Share one text-layout calculation between wrapped notes rendering and
  up/down cursor movement.
- [x] c) Sanitize every rendered edit buffer, including malformed titles,
  without invalidating byte offsets.
- [x] d) Introduce one bounded modal layout helper that clamps to the available
  area and always renders `Clear` before Browse, Move, and Keyboard content.
- [x] e) Add a compact fallback or minimum-size message when a details shelf
  cannot be displayed. Never capture input in an invisible editor.
- [x] f) Reserve space for Notes and Updated in the details shelf; cap,
  aggregate, or scroll large metadata/link sections.
- [x] g) Make Keyboard a static command table rather than a selectable list
  whose selection has no action.
- [x] h) Normalize key events and require exact modifiers. Handle `Ctrl-C`
  deliberately and prevent modified characters from triggering create,
  delete, confirmation, or refresh commands.

## Phase 8: Consolidate presentation and validation

Remove CLI/TUI dependency inversion and prevent malformed human-readable
output.

- [x] a) Extract Markdown formatting from [`src/cli.rs`](src/cli.rs) into a
  presentation module shared by CLI output and TUI yank.
- [x] b) Share title normalization and validation between CLI and TUI paths.
- [x] c) Strip unsafe terminal controls from human-readable output while
  leaving JSON semantically unchanged.
- [x] d) Escape Markdown headings, list text, code spans, labels, and link
  destinations according to context.
- [x] e) Replace the ad hoc URL scanner with parsing that preserves valid
  balanced parentheses and rejects malformed HTTP(S) URLs.
- [x] f) Document an explicit CLI/TUI capability matrix and either align the
  missing operations or mark them intentionally interface-specific.

## Phase 9: Split the TUI along tested boundaries

Perform this extraction after the state model is stable. Do not introduce
service or repository layers that own no rule or external contract.

- [ ] a) Move canonical board rules and row derivation into `tui/board.rs`.
- [ ] b) Move pure application transitions into `tui/app.rs`.
- [ ] c) Move command execution and worker protocol into `tui/worker.rs`.
- [ ] d) Move ratatui-only layout and drawing into `tui/render.rs`.
- [ ] e) Move wrapping, Unicode layout, and sanitization into `tui/text.rs`.
- [ ] f) Keep `tui/mod.rs` responsible only for startup, terminal lifecycle,
  event polling, and orchestration.
- [ ] g) Add a terminal guard that restores raw mode and keyboard enhancement
  flags on errors and panics.

## Phase 10: Lock in quality and remove drift

Add tests around the boundaries that currently have no meaningful coverage,
then make them required checks.

- [ ] a) Add reducer tests for sync failures, mutation rollback, stale refresh
  generations, delayed responses, draft recovery, and graceful shutdown.
- [ ] b) Add worker tests for ordered moves, partial moves, operation context,
  and channel closure.
- [ ] c) Add scripted HTTP tests for paths, query parameters, pagination,
  assigned tasks, encoding, token refresh, 401, 429, 5xx, timeouts, and
  malformed responses.
- [ ] d) Add filesystem tests for atomic token/cache writes, permissions,
  malformed files, account mismatch, save failures, and concurrent writers.
- [ ] e) Add render tests for every modal, narrow terminals, metadata overflow,
  CJK, emoji, combining characters, and control characters during editing.
- [ ] f) Stop tests from mutating process-wide environment variables without
  restoration or isolation.
- [ ] g) Make `cargo fmt --check`, strict Clippy, and the complete test suite
  required CI checks; fix the current formatting and lint failures.
- [ ] h) Remove completed work from the roadmap in [`README.md`](README.md),
  link this document there, and reduce [`CLAUDE.md`](CLAUDE.md) to durable
  architecture and safety invariants.

## Deferred feature work

Resume feature development after the reliability and architecture phases
above:

- Edit task due dates in the TUI.
- Manage task lists in the TUI.
- Expose task moves through the CLI.
- Add an Omarchy `theme-set.d` hook for immediate theme reload.
- Honor light-mode theme metadata.
- Reassess `toml` and ratatui upgrades against the chosen MSRV.
