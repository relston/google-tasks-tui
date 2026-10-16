# google-tasks-tui

Rust Google Tasks client with a ratatui TUI and a scriptable CLI.

Work directly on `main`; do not create feature branches.

## Architecture

Code is organized in three layers: task rules with no I/O, edges (Google,
disk, desktop), and the two front ends. Each file name says what it does.

- [`src/model.rs`](src/model.rs) owns the task types, their Google wire
  formats, and the title and URL rules both interfaces share. Use enums such
  as `Status`, not strings, for fixed states.
- [`src/board.rs`](src/board.rs) owns canonical task data, shared ordering,
  and tree logic (siblings, parent candidates, reorder plans). Rendered rows
  are projections, never persisted state.
- [`src/google/`](src/google/mod.rs) is the Google Tasks boundary. Both
  interfaces use `TasksApi`; keep request bodies typed and operation
  responses authoritative. [`google/auth.rs`](src/google/auth.rs) owns OAuth
  and token refresh; [`google/http.rs`](src/google/http.rs) owns the bounded
  HTTP agent.
- [`src/keys/`](src/keys/mod.rs) owns key parsing, terminal-event
  normalization, contextual conflict detection, and active labels. Each
  action is one line in the `actions!` table in
  [`keys/actions.rs`](src/keys/actions.rs) (name, contexts, defaults, help
  text); the enum, lookups, and the `[keys]` part of `--default-config` are
  generated from it. To add an action, add one table line and handle it in
  [`tui/input.rs`](src/tui/input.rs). [`src/config.rs`](src/config.rs) reads
  `[keys]` overrides by name; input dispatch, help, and status hints must all
  use the resolved keymap rather than hard-coded keys.
- The TUI keeps `App` state in [`src/tui/app.rs`](src/tui/app.rs) and splits
  behavior into `impl App` blocks by responsibility:
  [`input.rs`](src/tui/input.rs) dispatches keys,
  [`commands.rs`](src/tui/commands.rs) runs user commands,
  [`mutation.rs`](src/tui/mutation.rs) owns the in-flight mutation and
  rollback, [`sync.rs`](src/tui/sync.rs) owns refresh and recovery, and
  [`columns.rs`](src/tui/columns.rs) owns visible rows and selection.
  [`worker.rs`](src/tui/worker.rs) owns blocking API execution.
  [`view/`](src/tui/view/mod.rs) only lays out and draws.
- `Picker` backs every modal list (Browse, Move, Select Parent, Keyboard). Rows carry display
  text plus an opaque `value`, so one `draw_picker` and one `picker_key` serve
  all three. `/` filters by case-insensitive substring over title and subtitle;
  selection resets to the top on every query change so it can never point at a
  hidden row. Keep presentation out of app state: rows hold `String`s, not
  `Line`s.
- [`src/paths.rs`](src/paths.rs) owns path discovery. Build `Paths` once at
  the entry point and inject it; tests use temporary roots and never mutate
  process-wide environment variables.
- [`src/atomic_file.rs`](src/atomic_file.rs) is the only private atomic-write
  implementation. Token and cache files must remain mode `0600`.
- [`src/cache.rs`](src/cache.rs) is a disposable, versioned, account-scoped
  startup preview. Only confirmed state after a complete sync may be saved.
- Browser and clipboard effects belong in [`src/desktop.rs`](src/desktop.rs);
  Markdown output belongs in [`src/markdown.rs`](src/markdown.rs).
- [`src/theme.rs`](src/theme.rs) resolves semantic roles in layers: terminal
  ANSI defaults, then Omarchy (`source = "auto"`) for roles with no ANSI
  equivalent, then `[theme]` overrides. It then fits colors to the terminal
  (`NO_COLOR`, 256/16-color fallback). Hue roles stay ANSI; render
  selections through `Theme::selection()` so they survive `NO_COLOR`.

## Safety invariants

- Keep the board read-only until every list in the active refresh generation
  succeeds. Ignore stale-generation responses.
- Serialize mutations. Every mutation goes through `App::submit` (or
  `submit_create`): it copies the affected lists, applies the change on
  screen, and sends one `Op` to the worker. The worker replies with an
  `Outcome` of Google's authoritative lists and tasks; a failure restores the
  copies (and any new-task draft) and returns the app to read-only recovery.
  Add new mutations as `Op` variants, not new message types.
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
- Same-list reparenting and reordering are both `Op::Place`: `tasks.move`
  without `destinationTasklist`, followed by a reload of the list. Google
  stores positions as dense counters and renumbers siblings on every move, so
  the reload replaces the optimistic positions. Never patch sibling positions
  by guessing.
- Parent candidates are other open top-level tasks that Google permits as
  parents; selection follows the task ID. Tasks with children and
  completed-hidden tasks cannot become subtasks. Unparenting sets `previous`
  to the former parent so promotion does not jump to the top of the list.
- Reorders move a task among its open siblings under the same parent
  (`previous` = the sibling to land after, none for first).
  Presses made while the same task's reorder is saving move it locally and
  are sent as one follow-up move once the save lands. A failure restores the
  list from before the first press and drops the queued presses.
- Hiding completed tasks keeps a completed parent visible while it has open
  subtasks. Google scopes subtask positions to their parent, so a subtask is
  never shown among top-level tasks.
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
