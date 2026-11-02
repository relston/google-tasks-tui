use std::collections::HashMap;

use crate::{
    cache,
    model::{Task, TaskList},
};

#[derive(Clone)]
pub struct Row {
    pub task: Task,
    pub depth: u8,
}

#[derive(Clone, Default)]
pub struct Board {
    list_order: Vec<String>,
    lists: HashMap<String, TaskList>,
    task_order: HashMap<String, Vec<String>>,
    tasks: HashMap<String, HashMap<String, Task>>,
}

impl Board {
    pub fn from_cache(cache: cache::Cache) -> Self {
        let mut board = Self::default();
        for cached in cache.lists {
            let (order, tasks) = keyed(cached.tasks);
            board.list_order.push(cached.list.id.clone());
            board.task_order.insert(cached.list.id.clone(), order);
            board.tasks.insert(cached.list.id.clone(), tasks);
            board.lists.insert(cached.list.id.clone(), cached.list);
        }
        board
    }

    pub fn snapshot(&self) -> cache::Cache {
        cache::Cache {
            lists: self
                .list_order
                .iter()
                .filter_map(|id| {
                    Some(cache::CachedList {
                        list: self.lists.get(id)?.clone(),
                        tasks: self.rows(id).into_iter().map(|row| row.task).collect(),
                    })
                })
                .collect(),
        }
    }

    pub fn rows(&self, list_id: &str) -> Vec<Row> {
        ordered(self.tasks(list_id))
    }

    pub fn lists(&self) -> impl Iterator<Item = &TaskList> {
        self.list_order.iter().filter_map(|id| self.lists.get(id))
    }

    pub fn list(&self, list_id: &str) -> Option<&TaskList> {
        self.lists.get(list_id)
    }

    pub fn task(&self, list_id: &str, task_id: &str) -> Option<&Task> {
        self.tasks.get(list_id)?.get(task_id)
    }

    /// A task's direct subtasks, in no particular order.
    pub fn subtasks<'a>(
        &'a self,
        list_id: &str,
        task_id: &'a str,
    ) -> impl Iterator<Item = &'a Task> + 'a {
        self.tasks
            .get(list_id)
            .into_iter()
            .flat_map(|tasks| tasks.values())
            .filter(move |task| task.parent.as_deref() == Some(task_id))
    }

    pub fn task_mut(&mut self, list_id: &str, task_id: &str) -> Option<&mut Task> {
        self.tasks.get_mut(list_id)?.get_mut(task_id)
    }

    pub fn tasks(&self, list_id: &str) -> Vec<Task> {
        let Some(tasks) = self.tasks.get(list_id) else {
            return Vec::new();
        };
        let mut output: Vec<Task> = self
            .task_order
            .get(list_id)
            .into_iter()
            .flatten()
            .filter_map(|id| tasks.get(id))
            .cloned()
            .collect();
        let mut extras: Vec<&Task> = tasks
            .values()
            .filter(|task| {
                !self
                    .task_order
                    .get(list_id)
                    .is_some_and(|ids| ids.contains(&task.id))
            })
            .collect();
        extras.sort_by(|left, right| left.id.cmp(&right.id));
        output.extend(extras.into_iter().cloned());
        output
    }

    pub fn replace_lists(&mut self, lists: Vec<TaskList>) {
        let ids: Vec<String> = lists.iter().map(|list| list.id.clone()).collect();
        self.lists = lists
            .into_iter()
            .map(|list| (list.id.clone(), list))
            .collect();
        self.tasks.retain(|id, _| self.lists.contains_key(id));
        self.task_order.retain(|id, _| self.lists.contains_key(id));
        for id in &ids {
            self.tasks.entry(id.clone()).or_default();
            self.task_order.entry(id.clone()).or_default();
        }
        self.list_order = ids;
    }

    pub fn upsert_list(&mut self, list: TaskList) {
        if !self.lists.contains_key(&list.id) {
            self.list_order.push(list.id.clone());
            self.tasks.insert(list.id.clone(), HashMap::new());
            self.task_order.insert(list.id.clone(), Vec::new());
        }
        self.lists.insert(list.id.clone(), list);
    }

    pub fn remove_list(&mut self, list_id: &str) {
        self.list_order.retain(|id| id != list_id);
        self.lists.remove(list_id);
        self.task_order.remove(list_id);
        self.tasks.remove(list_id);
    }

    pub fn replace_tasks(&mut self, list_id: &str, tasks: Vec<Task>) {
        let (order, tasks) = keyed(tasks);
        self.task_order.insert(list_id.to_string(), order);
        self.tasks.insert(list_id.to_string(), tasks);
    }

    pub fn upsert_task(&mut self, list_id: &str, task: Task) -> bool {
        let Some(tasks) = self.tasks.get_mut(list_id) else {
            return false;
        };
        if !tasks.contains_key(&task.id) {
            self.task_order
                .entry(list_id.to_string())
                .or_default()
                .push(task.id.clone());
        }
        tasks.insert(task.id.clone(), task);
        true
    }

    pub fn remove_tree(&mut self, list_id: &str, task_id: &str) -> Vec<Task> {
        let source_order = self.task_order.get(list_id).cloned().unwrap_or_default();
        let Some(tasks) = self.tasks.get_mut(list_id) else {
            return Vec::new();
        };
        let ids: Vec<String> = source_order
            .into_iter()
            .filter(|id| {
                tasks.get(id).is_some_and(|task| {
                    task.id == task_id || task.parent.as_deref() == Some(task_id)
                })
            })
            .collect();
        if let Some(order) = self.task_order.get_mut(list_id) {
            order.retain(|id| !ids.contains(id));
        }
        ids.into_iter().filter_map(|id| tasks.remove(&id)).collect()
    }

    pub fn restore(&mut self, list_id: &str, tasks: Vec<Task>) {
        let Some(current) = self.tasks.get_mut(list_id) else {
            return;
        };
        let order = self.task_order.entry(list_id.to_string()).or_default();
        for task in tasks {
            if !current.contains_key(&task.id) {
                order.push(task.id.clone());
            }
            current.insert(task.id.clone(), task);
        }
    }

    pub fn move_tree(&mut self, from: &str, to: &str, task_id: &str) -> Vec<Task> {
        if !self.tasks.contains_key(to) {
            return Vec::new();
        }
        let moved = self.remove_tree(from, task_id);
        let moved_ids: std::collections::HashSet<String> =
            moved.iter().map(|task| task.id.clone()).collect();
        let moved: Vec<Task> = moved
            .into_iter()
            .map(|mut task| {
                if task
                    .parent
                    .as_deref()
                    .is_some_and(|parent| !moved_ids.contains(parent))
                {
                    task.parent = None;
                }
                task
            })
            .collect();
        self.restore(to, moved.clone());
        moved
    }
}

fn keyed(tasks: Vec<Task>) -> (Vec<String>, HashMap<String, Task>) {
    let tasks: Vec<Task> = tasks.into_iter().filter(|task| !task.deleted).collect();
    let order = tasks.iter().map(|task| task.id.clone()).collect();
    let tasks = tasks
        .into_iter()
        .map(|task| (task.id.clone(), task))
        .collect();
    (order, tasks)
}

pub fn ordered(tasks: Vec<Task>) -> Vec<Row> {
    let mut tasks: Vec<Task> = tasks.into_iter().filter(|task| !task.deleted).collect();
    tasks.sort_by(|a, b| {
        a.is_done().cmp(&b.is_done()).then_with(|| {
            if a.is_done() {
                b.completed.cmp(&a.completed)
            } else {
                a.position.cmp(&b.position)
            }
        })
    });
    let ids: std::collections::HashSet<String> = tasks.iter().map(|task| task.id.clone()).collect();
    let (children, top): (Vec<Task>, Vec<Task>) = tasks.into_iter().partition(|task| {
        task.parent
            .as_ref()
            .is_some_and(|parent| ids.contains(parent))
    });

    let mut rows = Vec::with_capacity(top.len() + children.len());
    for task in top {
        let id = task.id.clone();
        rows.push(Row { task, depth: 0 });
        rows.extend(
            children
                .iter()
                .filter(|child| child.parent.as_deref() == Some(id.as_str()))
                .cloned()
                .map(|task| Row { task, depth: 1 }),
        );
    }
    rows
}

pub fn eligible_parents(rows: &[Row], task_id: &str) -> Vec<Task> {
    rows.iter()
        .filter(|row| {
            row.depth == 0
                && row.task.id != task_id
                && !row.task.is_done()
                && !row.task.hidden
                && row.task.assignment_info.is_none()
        })
        .map(|row| row.task.clone())
        .collect()
}

/// Why this task cannot become a subtask, if it cannot.
pub fn subtask_block(rows: &[Row], task_id: &str) -> Option<&'static str> {
    let row = rows.iter().find(|row| row.task.id == task_id)?;
    if row.task.parent.is_some() {
        return Some("unparent this task before choosing a new parent");
    }
    if row.task.assignment_info.is_some() {
        return Some("assigned tasks cannot become subtasks");
    }
    if rows
        .iter()
        .any(|row| row.task.parent.as_deref() == Some(task_id))
    {
        return Some("a task with subtasks cannot become a subtask");
    }
    if row.task.hidden && row.task.is_done() {
        return Some("a completed hidden task cannot become a subtask");
    }
    None
}

/// Why this task (and its subtasks) cannot move to another list, if they cannot.
pub fn cross_list_block(rows: &[Row], task_id: &str) -> Option<&'static str> {
    let selected = rows.iter().find(|row| row.task.id == task_id)?;
    if selected.task.assignment_info.is_some()
        || rows.iter().any(|row| {
            row.task.parent.as_deref() == Some(task_id) && row.task.assignment_info.is_some()
        })
    {
        return Some("assigned tasks cannot move between lists");
    }
    None
}

/// A reorder: the task's parent, the open sibling Google should place it
/// after (`None` means first), and the position changes that show it there.
#[derive(Debug, PartialEq, Eq)]
pub struct ReorderPlan {
    pub parent: Option<String>,
    pub previous: Option<String>,
    /// `(task id, position)` for every sibling whose position changes.
    pub positions: Vec<(String, String)>,
}

/// The task's parent, its open siblings in board order (itself included), and
/// its index among them. Completed tasks are ordered by completion, so they
/// are never siblings.
fn open_siblings<'a>(
    rows: &'a [Row],
    task_id: &str,
) -> Result<(Option<String>, Vec<&'a Task>, usize), &'static str> {
    let Some(selected) = rows.iter().find(|row| row.task.id == task_id) else {
        return Err("task disappeared before it could move");
    };
    if selected.task.is_done() {
        return Err("completed tasks stay in completion order");
    }
    // A depth-0 row whose parent isn't in the list is shown as top-level.
    let parent = (selected.depth > 0)
        .then(|| selected.task.parent.clone())
        .flatten();
    let siblings: Vec<&Task> = rows
        .iter()
        .filter(|row| {
            row.depth == selected.depth
                && !row.task.is_done()
                && (selected.depth == 0 || row.task.parent == parent)
        })
        .map(|row| &row.task)
        .collect();
    let index = siblings
        .iter()
        .position(|task| task.id == task_id)
        .ok_or("task disappeared before it could move")?;
    Ok((parent, siblings, index))
}

/// The open sibling just before the task (`None` when it is first).
pub fn previous_sibling(rows: &[Row], task_id: &str) -> Result<Option<String>, &'static str> {
    let (_, siblings, index) = open_siblings(rows, task_id)?;
    Ok(index.checked_sub(1).map(|i| siblings[i].id.clone()))
}

/// Move the task one place up or down among its open siblings.
pub fn reorder_step(rows: &[Row], task_id: &str, down: bool) -> Result<ReorderPlan, &'static str> {
    let (_, siblings, index) = open_siblings(rows, task_id)?;
    let previous = if down {
        let next = siblings.get(index + 1).ok_or("task is already last")?;
        Some(next.id.as_str())
    } else {
        let above = index.checked_sub(1).ok_or("task is already first")?;
        above.checked_sub(1).map(|i| siblings[i].id.as_str())
    };
    reorder_after(rows, task_id, previous)
}

/// Put the task right after `previous` among its open siblings. Locally the
/// siblings' existing positions are handed out in the new order; Google's
/// renumbered list replaces them once the move lands.
pub fn reorder_after(
    rows: &[Row],
    task_id: &str,
    previous: Option<&str>,
) -> Result<ReorderPlan, &'static str> {
    let (parent, siblings, index) = open_siblings(rows, task_id)?;
    let mut order = siblings.clone();
    let moved = order.remove(index);
    let at = match previous {
        None => 0,
        Some(id) => {
            order
                .iter()
                .position(|task| task.id == id)
                .ok_or("its new neighbor is no longer beside it")?
                + 1
        }
    };
    order.insert(at, moved);
    let positions = order
        .iter()
        .zip(&siblings)
        .filter(|(task, slot)| task.position != slot.position)
        .map(|(task, slot)| (task.id.clone(), slot.position.clone()))
        .collect();
    Ok(ReorderPlan {
        parent,
        previous: previous.map(str::to_string),
        positions,
    })
}

/// The selected task, then its direct subtasks.
pub fn move_ids(rows: &[Row], task_id: &str) -> Vec<String> {
    if !rows.iter().any(|row| row.task.id == task_id) {
        return Vec::new();
    }
    let mut ids = vec![task_id.to_string()];
    ids.extend(
        rows.iter()
            .filter(|row| row.task.parent.as_deref() == Some(task_id))
            .map(|row| row.task.id.clone()),
    );
    ids
}

/// Google Tasks permits only one level of nesting.
pub fn subtask_parent(row: &Row) -> Option<String> {
    match row.depth {
        0 => Some(row.task.id.clone()),
        _ => row.task.parent.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Status;

    fn task(id: &str, parent: Option<&str>, position: &str, done: bool) -> Task {
        Task {
            id: id.into(),
            parent: parent.map(Into::into),
            position: position.into(),
            status: if done {
                Status::Completed
            } else {
                Status::NeedsAction
            },
            ..Default::default()
        }
    }

    #[test]
    fn board_keys_tasks_and_derives_the_shared_row_order() {
        let board = Board::from_cache(cache::Cache {
            lists: vec![cache::CachedList {
                list: TaskList {
                    id: "list".into(),
                    title: "Inbox".into(),
                    ..Default::default()
                },
                tasks: vec![
                    task("done", None, "0", true),
                    task("b", None, "2", false),
                    task("a1", Some("a"), "1", false),
                    task("a", None, "1", false),
                    Task {
                        deleted: true,
                        ..task("deleted", None, "3", false)
                    },
                ],
            }],
        });

        let rows = board.rows("list");
        let got: Vec<(&str, u8)> = rows
            .iter()
            .map(|row| (row.task.id.as_str(), row.depth))
            .collect();
        assert_eq!(got, [("a", 0), ("a1", 1), ("b", 0), ("done", 0)]);

        let snapshot = board.snapshot();
        assert_eq!(snapshot.lists[0].tasks.len(), 4);
    }

    #[test]
    fn board_moves_and_restores_a_task_tree() {
        let mut board = Board::from_cache(cache::Cache {
            lists: vec![
                cache::CachedList {
                    list: TaskList {
                        id: "from".into(),
                        title: "From".into(),
                        ..Default::default()
                    },
                    tasks: vec![
                        task("parent", None, "1", false),
                        task("child", Some("parent"), "2", false),
                    ],
                },
                cache::CachedList {
                    list: TaskList {
                        id: "to".into(),
                        title: "To".into(),
                        ..Default::default()
                    },
                    tasks: vec![task("there", None, "1", false)],
                },
            ],
        });
        let from = board.tasks("from");
        let to = board.tasks("to");

        let moved = board.move_tree("from", "to", "parent");
        assert_eq!(moved.len(), 2);
        assert!(board.task("from", "parent").is_none());
        assert_eq!(
            board
                .task("to", "child")
                .and_then(|task| task.parent.as_deref()),
            Some("parent")
        );

        board.replace_tasks("from", from);
        board.replace_tasks("to", to);
        assert!(board.task("from", "parent").is_some());
        assert!(board.task("to", "parent").is_none());
    }

    #[test]
    fn equal_sort_keys_preserve_source_order() {
        let source = vec![
            task("first", None, "same", false),
            task("second", None, "same", false),
            task("third", None, "same", false),
        ];
        let board = Board::from_cache(cache::Cache {
            lists: vec![cache::CachedList {
                list: TaskList {
                    id: "list".into(),
                    title: "List".into(),
                    ..Default::default()
                },
                tasks: source,
            }],
        });

        assert_eq!(
            board
                .tasks("list")
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second", "third"]
        );
        assert_eq!(
            board
                .rows("list")
                .iter()
                .map(|row| row.task.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second", "third"]
        );
    }

    #[test]
    fn orders_open_then_done_with_subtasks_under_parents() {
        let rows = ordered(vec![
            task("done", None, "0", true),
            task("b", None, "2", false),
            task("a1", Some("a"), "1", false),
            task("a", None, "1", false),
            task("orphan", Some("gone"), "3", false),
        ]);
        let got: Vec<(&str, u8)> = rows.iter().map(|r| (r.task.id.as_str(), r.depth)).collect();
        assert_eq!(
            got,
            [("a", 0), ("a1", 1), ("b", 0), ("orphan", 0), ("done", 0)]
        );
    }

    #[test]
    fn move_ids_are_the_task_and_its_subtasks() {
        let rows = ordered(vec![
            task("a", None, "1", false),
            task("a1", Some("a"), "1", false),
            task("b", None, "2", false),
        ]);
        assert_eq!(move_ids(&rows, "a"), ["a", "a1"]);
        assert_eq!(move_ids(&rows, "a1"), ["a1"]);
        assert!(move_ids(&rows, "gone").is_empty());
    }

    #[test]
    fn subtask_block_covers_parent_children_assignment_and_hidden() {
        let rows = ordered(vec![
            task("top", None, "1", false),
            task("child", Some("top"), "1", false),
            task("available", None, "2", false),
            Task {
                hidden: true,
                ..task("hidden-done", None, "3", true)
            },
            Task {
                assignment_info: Some(crate::model::AssignmentInfo {
                    link_to_task: "https://example".into(),
                    surface_type: "DOCUMENT".into(),
                }),
                ..task("assigned", None, "4", false)
            },
        ]);
        assert_eq!(
            subtask_block(&rows, "child"),
            Some("unparent this task before choosing a new parent")
        );
        assert_eq!(
            subtask_block(&rows, "top"),
            Some("a task with subtasks cannot become a subtask")
        );
        assert_eq!(
            subtask_block(&rows, "assigned"),
            Some("assigned tasks cannot become subtasks")
        );
        assert_eq!(
            subtask_block(&rows, "hidden-done"),
            Some("a completed hidden task cannot become a subtask")
        );
        assert_eq!(subtask_block(&rows, "available"), None);
    }

    #[test]
    fn cross_list_block_rejects_assigned_tasks_and_subtasks() {
        let rows = ordered(vec![
            task("ok", None, "1", false),
            Task {
                assignment_info: Some(crate::model::AssignmentInfo {
                    link_to_task: "https://example".into(),
                    surface_type: "DOCUMENT".into(),
                }),
                ..task("assigned", None, "2", false)
            },
            task("parent", None, "3", false),
            Task {
                assignment_info: Some(crate::model::AssignmentInfo {
                    link_to_task: "https://example".into(),
                    surface_type: "DOCUMENT".into(),
                }),
                ..task("assigned-child", Some("parent"), "1", false)
            },
        ]);
        assert_eq!(cross_list_block(&rows, "ok"), None);
        assert_eq!(
            cross_list_block(&rows, "assigned"),
            Some("assigned tasks cannot move between lists")
        );
        assert_eq!(
            cross_list_block(&rows, "parent"),
            Some("assigned tasks cannot move between lists")
        );
    }

    #[test]
    fn reorder_steps_target_open_siblings_under_the_same_parent() {
        let rows = ordered(vec![
            task("a", None, "1", false),
            task("a1", Some("a"), "1", false),
            task("a2", Some("a"), "2", false),
            task("a3", Some("a"), "3", false),
            task("b", None, "2", false),
            task("b1", Some("b"), "1", false),
            task("c", None, "3", false),
            task("done", None, "0", true),
        ]);
        let shift = |id: &str, down: bool| {
            reorder_step(&rows, id, down).map(|plan| (plan.parent, plan.previous))
        };
        let ok = |parent: Option<&str>, previous: Option<&str>| {
            Ok((parent.map(Into::into), previous.map(Into::into)))
        };

        // Top-level tasks skip over subtasks and land after their neighbor.
        assert_eq!(shift("a", true), ok(None, Some("b")));
        assert_eq!(shift("c", false), ok(None, Some("a")));
        assert_eq!(shift("b", false), ok(None, None));
        // Subtasks only trade places under their own parent.
        assert_eq!(shift("a3", false), ok(Some("a"), Some("a1")));
        assert_eq!(shift("a2", false), ok(Some("a"), None));
        assert_eq!(shift("a1", true), ok(Some("a"), Some("a2")));
        // Edges, lone subtasks, and completed tasks don't move.
        assert_eq!(shift("a", false), Err("task is already first"));
        assert_eq!(shift("c", true), Err("task is already last"));
        assert_eq!(shift("a3", true), Err("task is already last"));
        assert_eq!(shift("b1", false), Err("task is already first"));
        assert_eq!(
            shift("done", false),
            Err("completed tasks stay in completion order")
        );
    }

    #[test]
    fn reorder_after_hands_sibling_positions_out_in_the_new_order() {
        let rows = ordered(vec![
            task("a", None, "1", false),
            task("b", None, "2", false),
            task("c", None, "3", false),
            task("d", None, "4", false),
        ]);
        let positions = |id: &str, previous: Option<&str>| {
            reorder_after(&rows, id, previous).map(|plan| plan.positions)
        };
        let moved = |pairs: &[(&str, &str)]| {
            Ok(pairs
                .iter()
                .map(|(id, position)| (id.to_string(), position.to_string()))
                .collect::<Vec<_>>())
        };

        assert_eq!(
            positions("a", Some("c")),
            moved(&[("b", "1"), ("c", "2"), ("a", "3")])
        );
        assert_eq!(
            positions("d", None),
            moved(&[("d", "1"), ("a", "2"), ("b", "3"), ("c", "4")])
        );
        assert_eq!(positions("b", Some("a")), moved(&[]));
        assert_eq!(
            positions("a", Some("gone")),
            Err("its new neighbor is no longer beside it")
        );
    }

    #[test]
    fn subtask_parent_is_the_row_or_its_own_parent() {
        let rows = ordered(vec![
            task("a", None, "1", false),
            task("a1", Some("a"), "1", false),
            task("b", None, "2", false),
            task("orphan", Some("gone"), "3", false),
        ]);
        let row = |id: &str| rows.iter().find(|r| r.task.id == id).unwrap();
        assert_eq!(subtask_parent(row("a")).as_deref(), Some("a"));
        assert_eq!(subtask_parent(row("b")).as_deref(), Some("b"));
        // A subtask hands over its own parent, so nesting stays one deep.
        assert_eq!(subtask_parent(row("a1")).as_deref(), Some("a"));
        // A task whose parent is missing from the list draws at depth 0, so it
        // parents the new subtask itself instead of pointing at a stranger.
        assert_eq!(subtask_parent(row("orphan")).as_deref(), Some("orphan"));
    }
}
