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
| `tasklists.insert` | Create a new task list. | — | ✓ |
| `tasklists.patch` | Partially update a task list (title). | — | ✓ |
| `tasklists.update` | Fully replace a task list. | — | — |
| `tasklists.delete` | Delete a task list and all of its tasks. | — | ✓ |
| `tasks.list` | List every task in a task list. | ✓ | ✓ |
| `tasks.get` | Fetch one task by id. | — | — |
| `tasks.insert` | Create a task (optionally under a parent). | ✓ | ✓ |
| `tasks.patch` | Partially update a task. | ✓ | ✓ |
| `tasks.update` | Fully replace a task. | — | — |
| `tasks.delete` | Delete a task and its subtasks. | ✓ | ✓ |
| `tasks.move` | Reorder, reparent, or move a task across lists. | ✓ | — |
| `tasks.clear` | Remove all completed tasks from a list. | — | — |

## Notes

- **`tasklists.get` / `tasks.get`:** Neither surface calls these. The TUI and
  CLI load lists and tasks with `*.list` and select locally. CLI `get` prints
  one task that way; the TUI details shelf does the same.
- **`tasklists.update` / `tasks.update`:** Unused. Title, notes, due date, and
  status changes go through `*.patch`, which is enough for every field we
  edit.
- **`tasks.move`:** The TUI reorders, reparents, and moves across lists. The
  CLI can only set a parent on `create`; exposing moves is task 7 in
  [`TASKS.md`](TASKS.md).
- **List management:** Create, rename, and delete lists are CLI-only today
  (task 6 for the TUI).
- **`tasks.clear`:** Unsupported in both interfaces. Hiding completed tasks
  in the TUI is local display only and does not clear them in Google.
