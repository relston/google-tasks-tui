# Architecture Simplification Plan

The app has the features we want. Due-date editing is the only main feature
left, and it comes after this plan. This plan makes the code smaller and
easier to change without changing behavior, except for the fixes it calls out.

Complete phases in order. Write each test first and confirm that it fails for
the expected reason. End every phase with the required checks from
[`CLAUDE.md`](CLAUDE.md), `cargo install --path .`, and one commit on `main`.

## Target design

### Organize by responsibility

Three layers: task rules with no I/O, edges (Google, disk, desktop), and the
two front ends. Every file name says what the file does.

```text
src/
  main.rs, lib.rs        build Paths once, pick CLI or TUI
  model.rs               Task, TaskList, Status, due date, title rules
  board.rs               canonical store, ordering, tree queries and plans
  google/
    mod.rs               TasksApi trait, typed Error
    client.rs            Client, retries, endpoints
    requests.rs          CreateTask, TaskPatch, MoveTask, ListPatch
    auth.rs              OAuth and token refresh
    http.rs              agent, timeouts, URL encoding
  keys/
    actions.rs           the one action table
    combo.rs             parse, format, match key combos
    keymap.rs            resolve keys, conflict diagnostics
  paths.rs, atomic_file.rs, cache.rs, config.rs
  theme.rs, markdown.rs, desktop.rs
  cli.rs
  tui/
    mod.rs               start, event loop, terminal guard
    app.rs               App state and top-level update only
    input.rs             key -> action per context -> handler
    commands.rs          user intents -> operations
    mutation.rs          pending operation: submit, finish, rollback
    sync.rs              refresh generations, load state, recovery, cache save
    columns.rs           Column, visible rows, keeping the selection
    editor.rs            Edit buffer and cursor
    picker.rs            Picker and filtering
    worker.rs            operation -> API calls -> outcome
    view/                layout, columns, shelf, overlay, status, text
```

Renames: `storage.rs` becomes `atomic_file.rs`, `platform.rs` becomes
`desktop.rs`, and `presentation.rs` becomes `markdown.rs` (title rules move to
`model.rs`). [`src/tui/board.rs`](src/tui/board.rs) merges into
[`src/board.rs`](src/board.rs) so tree logic (siblings, parents, reorder and
move plans) has no TUI dependency, which also prepares CLI moves.

### One mutation lifecycle

Every mutation uses one operation type, one outcome type, and a snapshot of
the affected lists for rollback.

```rust
enum Op {
    Patch { list: String, task: String, patch: TaskPatch, etag: String },
    Create { list: String, task: CreateTask, parent: Option<String> },
    Delete { list: String, task: String, etag: String },
    MoveToList { from: String, to: String, ids: Vec<String> },
    Place { list: String, task: String, parent: Option<String>, previous: Option<String> },
}

struct Outcome {
    replaced: Vec<(String, Vec<Task>)>, // lists Google re-sent
    upserted: Vec<(String, Task)>,      // single tasks Google returned
    warning: Option<String>,            // partial move, reload failed
}
```

- `submit(op, affected_lists, optimistic_change)` copies the affected lists,
  applies the change on screen, and sends `Cmd::Run { id, op }`.
- The worker turns each API response into an `Outcome` and replies with
  `Msg::Finished { id, result }`. Moves reload their lists.
- `finish` matches only the operation ID. Success applies the outcome; failure
  restores the copied lists, reopens a create draft, and recovers.
- Reparent and reorder become one `Place` operation that reloads the list.

### One key action table

One macro line per action generates the enum, `ALL`, config names, help text,
contexts, default bindings, and the `[keys]` section of `--default-config`.

```rust
actions! {
    Quit            = "quit",             Board,          ["q"],                 "quit";
    ToggleCompleted = "toggle_completed", Board,          ["v"],                 "show / hide completed";
    ReorderDown     = "reorder_down",     Board,          ["alt+j", "alt+down"], "move task down one place";
    Help            = "help",             Board | Picker, ["shift+slash"],       "keyboard commands";
}
```

Dispatch stays a `match` per context in `input.rs`. `[keys]` is read as a map
and checked against the table, so an unknown name is a warning instead of a
reason to reset the whole config.

### Read-only rendering

Rendering takes `&App` and a small `ViewState` that holds only list scroll
offsets. State fix-ups (focus clamp, shelf close, orphaned edit) run after
each update, not during drawing.

### What stays and what to avoid

Keep the single worker thread, one mutation in flight, refresh generations,
the terminal guard, and the safe-write cache. Do not add a separate domain
model mapped from API types, async, or trait-object commands; each adds
ceremony without removing any.

# Tasks

## Phase 1: Easy wins

Replace strings and flag clusters with enums, delete unused code, and fix the
small rule breaks found in review.

- [x] a) Add a `Status` enum for `Task.status` and `TaskPatch.status`; use clap's value enum for `--status`.
- [x] b) Type `due` as `Option<NaiveDate>` and `completed` / `updated` as timestamps, parsed once on load.
- [x] c) Replace `Box<dyn Error>` and the `SignInExpired` downcast in [`src/auth.rs`](src/auth.rs) with an auth error enum.
- [x] d) Add `QuitState`, `Shelf::{Closed, Open(FieldId)}`, and `PickerStyle`; replace `KeyResolution` with `Option<Action>`.
- [x] e) Delete `cache::load` / `cache::save`, `config::load`, the unreachable `map_ureq` status arms, and the stale keybindings module comment. (`web_view_link` stays: CLI JSON output passes it through as `webViewLink`.)
- [x] f) Build `Paths` once in [`src/lib.rs`](src/lib.rs); delete the `Auth::new`, `Auth::cached`, and `cli::execute_to` wrappers; return an exit code instead of calling `process::exit`.
- [x] g) Replace the `Board::snapshot` detour in CLI JSON output with direct ordering.
- [x] h) Replace `fs2` with `File::lock` and declare `rust-version` in [`Cargo.toml`](Cargo.toml).
- [x] i) Build the quit notice and the "terminal too small" messages from keymap labels instead of hard-coded `q` and `Esc`.
- [x] j) Make test `App` construction use injected temporary `Paths`, never the real home directory.

## Phase 2: Organize by responsibility

Move code into the target layout with no behavior change. Keep each move
mechanical so the diff is easy to review.

- [x] a) Create `model.rs` with `Task`, `TaskList`, `Status`, and the title rules.
- [x] b) Create `google/` (`client.rs`, `requests.rs`, `auth.rs`, `http.rs`, `mod.rs`); implement `TasksApi` directly on `Client` and delete the forwarding copies.
- [x] c) Merge `src/tui/board.rs` into [`src/board.rs`](src/board.rs). (`move_requests` went to the worker, its only user.)
- [x] d) Rename `storage.rs` to `atomic_file.rs`, `platform.rs` to `desktop.rs`, and `presentation.rs` to `markdown.rs`.
- [x] e) Split `src/keybindings.rs` into `keys/actions.rs`, `keys/combo.rs`, and `keys/keymap.rs`.
- [x] f) Split [`src/tui/app.rs`](src/tui/app.rs) into `editor.rs`, `picker.rs`, `columns.rs`, `sync.rs`, `mutation.rs`, `input.rs`, and `commands.rs`.
- [x] g) Split `src/tui/render.rs` into `view/` modules for layout, columns, shelf, overlay, status, and text.
- [x] h) Move each test next to the code it covers.
- [x] i) Update the architecture section of [`CLAUDE.md`](CLAUDE.md) and the links in [`README.md`](README.md).

## Phase 3: One mutation lifecycle

Replace the per-mutation `Cmd`, `Msg`, worker arm, `MutationContext`,
`on_msg` arm, and `rollback` arm with `Op`, `Outcome`, and snapshot rollback.

- [x] a) Write lifecycle tests: submit copies and applies; success applies the outcome; failure restores lists and drafts; a warning applies, then recovers.
- [x] b) Define `Op` and `Outcome`; make the worker `execute` an `Op` and reply with `Msg::Finished` carrying the typed `google::Error`.
- [x] c) Add the pending operation (ID, copied lists, draft, success follow-up, queued reorder) with `submit` and `finish`. (The draft doubles as the success follow-up: a created task is selected only if the selection has not moved since.)
- [x] d) Port patch, create, delete, and cross-list move to `submit`.
- [x] e) Port reparent and reorder to `Place` with a list reload; add a test that unparenting keeps its place after Google renumbers siblings.
- [x] f) Delete `MutationContext`, the per-operation `Cmd` / `Msg` variants, the response-matching checks, and the `rollback` match.
- [x] g) Update the mutation invariants in [`CLAUDE.md`](CLAUDE.md).

## Phase 4: One key action table

Define every action once and generate everything else from it.

- [x] a) Write tests: the generated template round-trips to the default keymap, and an unknown `[keys]` name warns while other settings still load.
- [x] b) Add the `actions!` table generating the enum, `ALL`, names, help text, contexts, defaults, and name lookup.
- [x] c) Make `Keymap` concrete over `Action`; delete the generic parameter, the test action type, and `BindingSpec.name`. (`BindingSpec` and `BindingSource` are gone too; `Keymap::new(overrides)` builds from the table.)
- [x] d) Read `[keys]` as a map validated against the table, with one diagnostic per bad entry.
- [x] e) Generate the `[keys]` section of `--default-config` from the table. (Output is byte-for-byte what the hand-written template produced.)
- [x] f) Replace the hand-maintained action list in [`README.md`](README.md) with pointers to `--default-config` and the `?` overlay.

## Phase 5: Read-only rendering

Make the compiler enforce that drawing does not change application state.

- [ ] a) Write tests: drawing twice leaves `App` unchanged, and an edit whose task disappears closes after the update with a visible message.
- [ ] b) Add `ViewState` for list scroll offsets and horizontal scroll; change rendering to take `&App` and `&mut ViewState`.
- [ ] c) Move focus clamping, shelf closing, and orphaned-edit handling into a post-update step in `columns.rs`.
- [ ] d) Replace the `edit_width` feedback with a pure layout function that uses the stored terminal size.
- [ ] e) Stop rebuilding the full task tree on every frame while the details shelf is open.
- [ ] f) Add the read-only rendering rule to [`CLAUDE.md`](CLAUDE.md).
