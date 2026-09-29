# google-tasks-tui

Rust Google Tasks client with a ratatui TUI and a scriptable CLI.

Work directly on `main`; do not create feature branches.

## Architecture

- [`src/api.rs`](src/api.rs) is the Google Tasks boundary. Both interfaces use
  `TasksApi`; keep request bodies typed and operation responses authoritative.
- [`src/board.rs`](src/board.rs) owns canonical task data and shared ordering.
  Rendered rows are projections, never persisted state.
- [`src/keybindings.rs`](src/keybindings.rs) owns action metadata, default
  bindings, parsing, terminal-event normalization, contextual conflict
  detection, and active labels. [`src/config.rs`](src/config.rs)
  provides optional `[keys]` overrides; input dispatch, help, and status hints
  must all use the resolved keymap rather than hard-coded keys.
- [`src/tui/app.rs`](src/tui/app.rs) owns pure UI transitions and optimistic
  rollback context. [`src/tui/worker.rs`](src/tui/worker.rs) owns blocking API
  execution. [`src/tui/render.rs`](src/tui/render.rs) only lays out and draws.
- `Picker` backs every modal list (Browse, Move, Select Parent, Keyboard). Rows carry display
  text plus an opaque `value`, so one `draw_picker` and one `picker_key` serve
  all three. `/` filters by case-insensitive substring over title and subtitle;
  selection resets to the top on every query change so it can never point at a
  hidden row. Keep presentation out of app state: rows hold `String`s, not
  `Line`s.
- [`src/paths.rs`](src/paths.rs) owns path discovery. Inject `Paths` or explicit
  roots in tests; never mutate process-wide environment variables.
- [`src/storage.rs`](src/storage.rs) is the only private atomic-write
  implementation. Token and cache files must remain mode `0600`.
- [`src/cache.rs`](src/cache.rs) is a disposable, versioned, account-scoped
  startup preview. Only confirmed state after a complete sync may be saved.
- [`src/auth.rs`](src/auth.rs) owns OAuth, token refresh, and bounded HTTP
  configuration. Browser and clipboard effects belong in
  [`src/platform.rs`](src/platform.rs).
- [`src/theme.rs`](src/theme.rs) maps Omarchy palettes to semantic roles and
  falls back to terminal ANSI colors when theme data is missing or invalid.

## Safety invariants

- Keep the board read-only until every list in the active refresh generation
  succeeds. Ignore stale-generation responses.
- Serialize mutations. Every optimistic mutation must retain enough context to
  roll back, and failures must return the app to read-only recovery.
- Preserve editor, selection, and operation identity across delayed worker
  responses. The `quit` action defaults to `q`; Escape defaults to
  close/cancel and never quits. Graceful shutdown waits for pending writes; a
  second invocation of the configured quit action force-quits.
- Keep keybindings context-specific. Unmodified printable bindings are valid
  in command contexts but must not intercept text entry in editors or picker
  filters. User overrides displace conflicting defaults; duplicate user
  bindings produce a visible diagnostic.
- Cross-list moves preserve parent/child order, report partial completion, and
  reconcile both source and destination lists.
- Same-list reparenting uses `tasks.move` without `destinationTasklist`.
  Parent candidates are other open top-level tasks that Google permits as
  parents; optimistic parent changes retain rollback context and task-ID
  selection. Tasks with children and completed-hidden tasks cannot become
  subtasks. Unparenting sets `previous` to the former parent so promotion does
  not jump to the top of the list.
- Bound all network operations. Retry only safe reads for 429/5xx, honor
  `Retry-After`, and retry at most once after an unexpected 401 refresh.
- Keep terminal text grapheme-aware and terminal-cell-aware. Sanitize controls
  before rendering without changing byte offsets used by editors.
- Always restore raw mode, alternate screen, and keyboard enhancement flags on
  normal return, errors, and panic unwinding.
- Missing themes and disposable cache failures must not crash the app. Remote
  mutation persistence failures must remain visible to the user.

## Verification

Run all required checks before completing work:

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
git diff --check
```
