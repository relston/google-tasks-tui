use crate::{
    api::{MoveTask, Task, TaskList},
    board::Row,
};

pub(super) fn other_lists<'a>(lists: &'a [TaskList], current_id: &str) -> Vec<&'a TaskList> {
    lists.iter().filter(|list| list.id != current_id).collect()
}

pub(super) fn eligible_parents(rows: &[Row], task_id: &str) -> Vec<Task> {
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

/// A one-step reorder: the sibling the task trades places with, and the
/// `previous` sibling Google should place it after (`None` means first).
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Shift {
    pub parent: Option<String>,
    pub neighbor: String,
    pub previous: Option<String>,
}

/// Siblings are the open tasks at the same depth under the same parent, in
/// board order. Completed tasks are ordered by completion, so they don't move.
pub(super) fn shift_target(rows: &[Row], task_id: &str, down: bool) -> Result<Shift, &'static str> {
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
    let siblings: Vec<&str> = rows
        .iter()
        .filter(|row| {
            row.depth == selected.depth
                && !row.task.is_done()
                && (selected.depth == 0 || row.task.parent == parent)
        })
        .map(|row| row.task.id.as_str())
        .collect();
    let Some(index) = siblings.iter().position(|id| *id == task_id) else {
        return Err("task disappeared before it could move");
    };
    let (neighbor, previous) = if down {
        let neighbor = siblings.get(index + 1).ok_or("task is already last")?;
        (*neighbor, Some(*neighbor))
    } else {
        let neighbor = index
            .checked_sub(1)
            .map(|i| siblings[i])
            .ok_or("task is already first")?;
        (neighbor, index.checked_sub(2).map(|i| siblings[i]))
    };
    Ok(Shift {
        parent,
        neighbor: neighbor.to_string(),
        previous: previous.map(str::to_string),
    })
}

/// The selected task, then its direct subtasks.
pub(super) fn move_ids(rows: &[Row], task_id: &str) -> Vec<String> {
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
pub(super) fn subtask_parent(row: &Row) -> Option<String> {
    match row.depth {
        0 => Some(row.task.id.clone()),
        _ => row.task.parent.clone(),
    }
}

pub(super) fn move_requests(destination: &str, ids: &[String]) -> Vec<(String, MoveTask)> {
    let Some(root) = ids.first() else {
        return Vec::new();
    };
    ids.iter()
        .enumerate()
        .map(|(index, id)| {
            (
                id.clone(),
                MoveTask {
                    destination_list: Some(destination.to_string()),
                    parent: (index > 0).then(|| root.clone()),
                    previous: (index > 1).then(|| ids[index - 1].clone()),
                },
            )
        })
        .collect()
}
