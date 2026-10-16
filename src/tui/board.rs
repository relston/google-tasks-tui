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

/// A reorder: the task's parent, the open sibling Google should place it
/// after (`None` means first), and the position changes that show it there.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ReorderPlan {
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
pub(super) fn previous_sibling(
    rows: &[Row],
    task_id: &str,
) -> Result<Option<String>, &'static str> {
    let (_, siblings, index) = open_siblings(rows, task_id)?;
    Ok(index.checked_sub(1).map(|i| siblings[i].id.clone()))
}

/// Move the task one place up or down among its open siblings.
pub(super) fn reorder_step(
    rows: &[Row],
    task_id: &str,
    down: bool,
) -> Result<ReorderPlan, &'static str> {
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
pub(super) fn reorder_after(
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
