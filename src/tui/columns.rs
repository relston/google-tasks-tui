//! Columns as shown: visible rows, selection by task ID, and moving the
//! cursor.

use std::collections::HashSet;

use crate::board::{Board, Row};
use crate::model::TaskList;

use super::app::{App, Interaction, Shelf};
use super::editor::{Edit, EditTarget};
use super::sync::LoadState;

pub(super) struct Column {
    pub(super) list: TaskList,
    pub(super) rows: Vec<Row>,
    pub(super) load: LoadState,
    pub(super) selected_id: Option<String>,
}

impl Column {
    pub(super) fn new(list: TaskList, rows: Vec<Row>, load: LoadState) -> Self {
        let selected_id = rows.first().map(|row| row.task.id.clone());
        Self {
            list,
            rows,
            load,
            selected_id,
        }
    }

    pub(super) fn selected(&self) -> Option<&Row> {
        let selected = self.selected_id.as_deref()?;
        self.rows.iter().find(|row| row.task.id == selected)
    }

    /// The selected task's row index, for drawing and stepping.
    pub(super) fn selected_index(&self) -> Option<usize> {
        let selected = self.selected_id.as_deref()?;
        self.rows.iter().position(|row| row.task.id == selected)
    }

    pub(super) fn select(&mut self, index: Option<usize>) {
        self.selected_id = index
            .and_then(|index| self.rows.get(index))
            .map(|row| row.task.id.clone());
    }

    pub(super) fn select_id(&mut self, task_id: Option<String>) {
        let index = task_id
            .as_deref()
            .and_then(|id| self.rows.iter().position(|row| row.task.id == id));
        self.select(index);
    }
}

/// Column rows. Hiding completed tasks keeps a completed parent while it has
/// open subtasks: Google scopes subtask positions to their parent, so those
/// subtasks have no meaningful place among the top-level tasks.
pub(super) fn visible_rows(board: &Board, list_id: &str, show_completed: bool) -> Vec<Row> {
    let rows = board.rows(list_id);
    if show_completed {
        return rows;
    }
    let open_parents: HashSet<String> = rows
        .iter()
        .filter(|row| row.depth > 0 && !row.task.is_done())
        .filter_map(|row| row.task.parent.clone())
        .collect();
    rows.into_iter()
        .filter(|row| !row.task.is_done() || open_parents.contains(&row.task.id))
        .collect()
}

impl App {
    pub(super) fn rebuild_column(&mut self, list_id: &str) {
        let rows = visible_rows(&self.board, list_id, self.show_completed);
        let Some(column) = self
            .columns
            .iter_mut()
            .find(|column| column.list.id == list_id)
        else {
            return;
        };
        let selected = column.selected_id.clone();
        let previous = column.selected_index().unwrap_or(0);
        column.rows = rows;
        if selected
            .as_deref()
            .is_some_and(|id| column.rows.iter().any(|row| row.task.id == id))
        {
            column.select_id(selected);
        } else {
            let last = column.rows.len().checked_sub(1);
            column.select(last.map(|last| previous.min(last)));
        }
    }

    pub(super) fn step_selection(&mut self, delta: isize) {
        let Some(col) = self.columns.get_mut(self.focus) else {
            return;
        };
        let Some(last) = col.rows.len().checked_sub(1) else {
            return;
        };
        let cur = col.selected_index().unwrap_or(0) as isize;
        col.select(Some((cur + delta).clamp(0, last as isize) as usize));
    }

    pub(super) fn selected_task(&self) -> Option<&Row> {
        self.columns.get(self.focus)?.selected()
    }

    /// Keep focus, the shelf, and an open edit consistent with the columns.
    /// Runs after every key and worker message, so drawing never has to.
    pub(super) fn settle(&mut self) {
        self.focus = self.focus.min(self.columns.len().saturating_sub(1));
        if let Interaction::ListEdit(edit) = &self.interaction {
            if !edit.is_create() {
                let still_there = self
                    .columns
                    .iter()
                    .any(|column| column.list.id == edit.list_id);
                if !still_there {
                    self.interaction = Interaction::Idle;
                    self.error =
                        Some("the list being edited disappeared; the rename was not saved".into());
                }
            }
        }
        if self.selected_task().is_some() || self.is_creating() {
            return;
        }
        self.shelf = Shelf::Closed;
        if matches!(
            self.interaction,
            Interaction::Editing(Edit {
                target: EditTarget::Existing { .. },
                ..
            }) | Interaction::DateEdit(_)
        ) {
            self.interaction = Interaction::Idle;
            self.error = Some("the task being edited disappeared; the edit was not saved".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::tui::test_support::*;

    #[test]
    fn completion_reorders_immediately_and_selection_follows_its_id() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("a-one", None, "1", false),
                task("a-two", None, "2", false),
            ],
            "a-one",
        );

        app.toggle_done();

        assert_eq!(
            app.columns[0]
                .rows
                .iter()
                .map(|row| row.task.id.as_str())
                .collect::<Vec<_>>(),
            ["a-two", "a-one"]
        );
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("a-one"));
        assert_eq!(app.columns[0].selected_index(), Some(1));
    }

    #[test]
    fn v_toggles_completed_tasks_on_every_list() {
        let (mut app, commands) = app_with_lists(&["a", "b"]);
        let _ = commands.recv().unwrap();
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("open", None, "1", false),
                task("done-child", Some("open"), "2", true),
                task("done-parent", None, "3", true),
                task("open-child", Some("done-parent"), "4", false),
            ],
            "done-child",
        );
        set_tasks(
            &mut app,
            "b",
            vec![
                task("open-b", None, "1", false),
                task("done-b", None, "2", true),
            ],
            "open-b",
        );
        app.shelf = Shelf::Open(FieldId::Title);

        app.on_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));

        assert_eq!(
            shown(&app, "a"),
            [
                ("open".into(), 0),
                ("done-parent".into(), 0),
                ("open-child".into(), 1),
            ]
        );
        assert_eq!(shown(&app, "b"), [("open-b".into(), 0)]);
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("done-parent"));
        assert_eq!(app.shelf, Shelf::Closed);
        assert!(app.board.task("a", "done-parent").unwrap().is_done());
        assert!(app.board.task("a", "done-child").unwrap().is_done());

        app.on_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));

        assert_eq!(
            shown(&app, "a"),
            [
                ("open".into(), 0),
                ("done-child".into(), 1),
                ("done-parent".into(), 0),
                ("open-child".into(), 1),
            ]
        );
        assert_eq!(
            shown(&app, "b"),
            [("open-b".into(), 0), ("done-b".into(), 0)]
        );
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("done-parent"));
    }

    #[test]
    fn hidden_completed_parents_keep_their_open_subtasks_in_place() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("top", None, "1", false),
                task("done-parent", None, "0", true),
                task("one", Some("done-parent"), "1", false),
                task("two", Some("done-parent"), "2", false),
                task("done", None, "2", true),
            ],
            "two",
        );

        app.on_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
        assert_eq!(
            shown(&app, "a"),
            [
                ("top".into(), 0),
                ("done-parent".into(), 0),
                ("one".into(), 1),
                ("two".into(), 1),
            ]
        );

        app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT));
        assert_eq!(
            shown(&app, "a"),
            [
                ("top".into(), 0),
                ("done-parent".into(), 0),
                ("two".into(), 1),
                ("one".into(), 1),
            ]
        );
        assert!(matches!(
            commands.recv().unwrap(),
            Cmd::Run {
                op: Op::Place {
                task: task_id,
                parent: Some(parent),
                previous: None,
                ..
            },
                ..
            } if task_id == "two" && parent == "done-parent"
        ));
    }

    #[test]
    fn an_edit_whose_task_disappears_closes_with_a_message() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        let generation = app.refresh_generation;
        app.shelf = Shelf::Open(FieldId::Title);
        app.interaction = Interaction::Editing(Edit {
            target: EditTarget::Existing {
                list_id: "a".into(),
                task_id: "a-cached".into(),
                field: FieldId::Title,
            },
            buf: "half typed".into(),
            cursor: 10,
        });
        let lists = app.columns.iter().map(|c| c.list.clone()).collect();

        app.on_msg(Msg::Lists(generation, Ok(lists)));
        app.on_msg(Msg::Tasks(generation, "a".into(), Ok(Vec::new())));

        assert!(matches!(app.interaction, Interaction::Idle));
        assert_eq!(app.shelf, Shelf::Closed);
        assert!(
            app.error
                .as_deref()
                .is_some_and(|error| error.contains("not saved")),
            "{:?}",
            app.error
        );
    }
}
