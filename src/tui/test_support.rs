//! Shared fixtures for TUI unit tests.

pub(super) use std::sync::mpsc::{self, Receiver};

pub(super) use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(super) use crate::cache;
pub(super) use crate::config::Config;
pub(super) use crate::google::TaskPatch;
pub(super) use crate::keys::BindingConfig;
pub(super) use crate::model::{Status, Task, TaskList};

pub(super) use super::app::*;
pub(super) use super::editor::*;
pub(super) use super::mutation::*;
pub(super) use super::picker::*;
pub(super) use super::sync::*;
pub(super) use super::worker::*;

pub(super) fn app_with_lists(ids: &[&str]) -> (App, Receiver<Cmd>) {
    app_with_lists_and_config(ids, Config::default())
}

pub(super) fn app_with_lists_and_config(ids: &[&str], config: Config) -> (App, Receiver<Cmd>) {
    let (tx, rx) = mpsc::channel();
    let cached = cache::Cache {
        lists: ids
            .iter()
            .map(|id| cache::CachedList {
                list: TaskList {
                    id: (*id).into(),
                    title: format!("List {id}"),
                    ..Default::default()
                },
                tasks: vec![task(&format!("{id}-cached"), None, "1", false)],
            })
            .collect(),
    };
    (App::new(tx, cached, config), rx)
}

pub(super) fn set_tasks(app: &mut App, list_id: &str, tasks: Vec<Task>, selected: &str) {
    app.board.replace_tasks(list_id, tasks);
    app.rebuild_column(list_id);
    let column = app
        .columns
        .iter_mut()
        .find(|column| column.list.id == list_id)
        .unwrap();
    column.select_id(Some(selected.into()));
}

pub(super) fn task(id: &str, parent: Option<&str>, pos: &str, done: bool) -> Task {
    Task {
        id: id.into(),
        parent: parent.map(Into::into),
        position: pos.into(),
        status: if done {
            Status::Completed
        } else {
            Status::NeedsAction
        },
        ..Default::default()
    }
}

pub(super) fn edit(buf: &str) -> Edit {
    Edit {
        target: EditTarget::Existing {
            list_id: "l".into(),
            task_id: "t".into(),
            field: FieldId::Title,
        },
        cursor: buf.len(),
        buf: buf.into(),
    }
}

pub(super) fn new_edit(list_id: &str, selection: Option<&str>, buf: &str) -> Edit {
    Edit {
        target: EditTarget::New {
            list_id: list_id.into(),
            parent: None,
            selection: selection.map(Into::into),
        },
        buf: buf.into(),
        cursor: buf.len(),
    }
}

pub(super) fn pending(app: &mut App, context: MutationContext) -> u64 {
    app.pending_operation = Some(PendingOperation { id: 1, context });
    1
}

pub(super) fn shown(app: &App, list_id: &str) -> Vec<(String, u8)> {
    app.columns
        .iter()
        .find(|column| column.list.id == list_id)
        .unwrap()
        .rows
        .iter()
        .map(|row| (row.task.id.clone(), row.depth))
        .collect()
}

pub(super) fn typed_keys(app: &mut App, keys: &str) {
    for c in keys.chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

pub(super) fn ids(app: &App, list_id: &str) -> Vec<String> {
    app.columns
        .iter()
        .find(|column| column.list.id == list_id)
        .unwrap()
        .rows
        .iter()
        .map(|row| row.task.id.clone())
        .collect()
}

pub(super) fn reordered(operation: u64, task_id: &str, list: &[&str]) -> Msg {
    let tasks: Vec<Task> = list
        .iter()
        .enumerate()
        .map(|(index, id)| task(id, None, &index.to_string(), false))
        .collect();
    let moved = tasks
        .iter()
        .find(|task| task.id == task_id)
        .unwrap()
        .clone();
    Msg::Reordered {
        operation,
        list_id: "a".into(),
        task_id: task_id.into(),
        parent: None,
        result: Ok(moved),
        tasks: Some(Ok(tasks)),
    }
}

pub(super) fn reorder_list(app: &mut App, ids: &[&str]) {
    let tasks = ids
        .iter()
        .enumerate()
        .map(|(index, id)| task(id, None, &index.to_string(), false))
        .collect();
    set_tasks(app, "a", tasks, ids[0]);
}
