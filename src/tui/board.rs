use crate::{
    api::{MoveTask, TaskList},
    board::Row,
};

pub(super) fn other_lists<'a>(lists: &'a [TaskList], current_id: &str) -> Vec<&'a TaskList> {
    lists.iter().filter(|list| list.id != current_id).collect()
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
                    destination_list: destination.to_string(),
                    parent: (index > 0).then(|| root.clone()),
                    previous: (index > 1).then(|| ids[index - 1].clone()),
                },
            )
        })
        .collect()
}
