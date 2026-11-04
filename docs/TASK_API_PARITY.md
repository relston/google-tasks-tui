# Google Tasks API parity

Coverage of [Google Tasks API v1](https://developers.google.com/workspace/tasks/reference/rest)
against this app's TUI and CLI. A check means the interface can do what that
method is for (create a list, move a task, and so on). Methods we never call
are still checked when an equivalent path exists; see the notes under the
table.

Service endpoint: `https://tasks.googleapis.com`

| Endpoint | Description | TUI | CLI |
| --- | --- | --- | --- |
| `tasklists.list` | List every task list for the signed-in user. | ✓ | ✓ |
| `tasklists.get` | Fetch one task list by id. | — | — |
| `tasklists.insert` | Create a new task list. | ✓ | ✓ |
| `tasklists.patch` | Partially update a task list (title). | ✓ | ✓ |
| `tasklists.update` | Fully replace a task list. | — | — |
| `tasklists.delete` | Delete a task list and all of its tasks. | ✓ | ✓ |
| `tasks.list` | List every task in a task list. | ✓ | ✓ |
| `tasks.get` | Fetch one task by id. | — | — |
| `tasks.insert` | Create a task (optionally under a parent). | ✓ | ✓ |
| `tasks.patch` | Partially update a task. | ✓ | ✓ |
| `tasks.update` | Fully replace a task. | — | — |
| `tasks.delete` | Delete a task and its subtasks. | ✓ | ✓ |
| `tasks.move` | Reorder, reparent, or move a task across lists. | ✓ | ✓ |
| `tasks.clear` | Remove all completed tasks from a list. | — | — |

## Notes

- **`tasklists.get` / `tasks.get`:** Neither surface calls these. The TUI and
  CLI load lists and tasks with `*.list` and select locally. CLI `get` prints
  one task that way; the TUI details shelf does the same.
- **`tasklists.update` / `tasks.update`:** Unused. Title, notes, due date, and
  status changes go through `*.patch`, which is enough for every field we
  edit.
- **`tasks.move`:** Both interfaces reorder, reparent, and move across lists.
  The CLI uses `move` with mutually exclusive flags (`--up` / `--down` /
  `--first` / `--after`, `--parent` / `--top-level`, `--to-list`).
- **List management:** The TUI creates (`Shift+t`), renames (`Shift+i`), and
  deletes (`Shift+x`) lists. Create and rename share the floating title panel.
  Reordering columns with `Shift+h` / `Shift+l` is local display order only —
  Google has no `tasklists.move`.
- **`tasks.clear`:** Unsupported in both interfaces. Hiding completed tasks
  in the TUI is local display only and does not clear them in Google.
- **Due time and recurrence:** The Task resource exposes `due` as a date
  only (any time portion is discarded). There is no recurrence field, so
  repeating tasks from the Tasks/Calendar UI cannot be read or written
  through the API. Tracked upstream as
  [issuetracker.google.com/issues/479850449](https://issuetracker.google.com/issues/479850449).
