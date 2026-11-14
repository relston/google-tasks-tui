//! What each user command does: validate it, update the board, and start
//! the mutation that tells Google.

use crate::board::{
    cross_list_block, eligible_parents, move_ids, reorder_after, reorder_step, subtask_block,
    subtask_parent, Board, ReorderPlan,
};
use crate::desktop::{self, Clipboard};
use crate::google::{CreateTask, TaskPatch};
use crate::keys::Action;
use crate::markdown::md_yank;
use crate::model::{normalize_title, urls_in, Status, Task, TaskList};

use super::app::{App, Interaction, QuitState, Shelf};
use super::columns::visible_rows;
use super::debug_date::{DateEdit, DateEditor};
use super::editor::{Edit, EditTarget, FieldId};
use super::list_edit::ListEdit;
use super::mutation::Reorder;
use super::picker::{Picker, PickerRow};
use super::sync::{SyncState, READ_ONLY};
use super::view::text::{clean, title_of};
use super::worker::Op;

/// What a y/n confirmation will delete or clear.
pub(super) enum PendingTarget {
    Task {
        list_id: String,
        task_id: String,
        etag: String,
    },
    List {
        list_id: String,
    },
    ClearCompleted {
        list_id: String,
    },
}

/// A delete waiting on a y/n answer in the status bar.
pub(super) struct Pending {
    pub(super) target: PendingTarget,
    pub(super) prompt: String,
}

/// One row of the Browse overlay.
pub(super) struct BrowseLink {
    /// The source's title, for links Google attached to the task. URLs picked
    /// out of the notes have nothing to call themselves.
    pub(super) name: Option<String>,
    pub(super) url: String,
}

/// Overlay listing possible parent tasks in the current list.
/// Reparenting needs the task being moved as well as the candidates, so the
/// picker carries the context the rows cannot.
pub(super) struct ParentPick {
    pub(super) list_id: String,
    pub(super) task_id: String,
    pub(super) picker: Picker,
}

fn set_positions(board: &mut Board, list_id: &str, positions: &[(String, String)]) {
    for (task_id, position) in positions {
        if let Some(task) = board.task_mut(list_id, task_id) {
            task.position = position.clone();
        }
    }
}

/// Everything the task links to: the links Google attached to it (a Gmail
/// thread, a Doc) keep the source's title, then any bare URLs typed into the
/// notes. First-seen order, no duplicate URLs.
pub(super) fn task_links(task: &Task) -> Vec<BrowseLink> {
    let mut out: Vec<BrowseLink> = Vec::new();
    for l in &task.links {
        let name = l.description.trim();
        if l.link.is_empty() || out.iter().any(|b| b.url == l.link) {
            continue;
        }
        out.push(BrowseLink {
            name: (!name.is_empty()).then(|| clean(name).into_owned()),
            url: clean(&l.link).into_owned(),
        });
    }
    for url in urls_in(task.notes.as_deref().unwrap_or("")) {
        if !out.iter().any(|b| b.url == url) {
            out.push(BrowseLink { name: None, url });
        }
    }
    out
}

pub(super) fn other_lists<'a>(lists: &'a [TaskList], current_id: &str) -> Vec<&'a TaskList> {
    lists.iter().filter(|list| list.id != current_id).collect()
}

impl App {
    /// Start a new task in the focused list: an empty title input in the shelf.
    /// `under` makes it a subtask of the selection (see `subtask_parent`).
    pub(super) fn start_create(&mut self, under: bool) {
        if !self.writable() {
            return;
        }
        let Some(col) = self.columns.get(self.focus) else {
            return;
        };
        let list_id = col.list.id.clone();
        let selected = col.selected().map(|row| row.task.id.clone());
        if under
            && col
                .selected()
                .is_some_and(|row| row.task.assignment_info.is_some())
        {
            self.error = Some("assigned tasks cannot have subtasks".into());
            return;
        }
        let parent = match (under, col.selected().map(subtask_parent)) {
            (false, _) => None,
            (true, Some(p)) => p,
            (true, None) => {
                self.error = Some("select a task to put the subtask under".into());
                return;
            }
        };
        self.interaction = Interaction::Editing(Edit {
            target: EditTarget::New {
                list_id,
                parent,
                selection: selected,
            },
            buf: String::new(),
            cursor: 0,
        });
        self.shelf = Shelf::Open(FieldId::Title);
    }

    /// Open the floating due-date editor for the selected task.
    pub(super) fn start_edit_due(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(col) = self.columns.get(self.focus) else {
            return;
        };
        let Some(row) = col.selected() else {
            return;
        };
        let due = row
            .task
            .due
            .unwrap_or_else(|| chrono::Local::now().date_naive());
        self.interaction = Interaction::DateEdit(DateEdit {
            list_id: col.list.id.clone(),
            task_id: row.task.id.clone(),
            editor: DateEditor::new(due),
        });
    }

    /// Apply the date editor's value to the selected task via `tasks.patch`.
    pub(super) fn commit_due(&mut self) {
        if !self.writable() {
            return;
        }
        let interaction = std::mem::replace(&mut self.interaction, Interaction::Idle);
        let Interaction::DateEdit(edit) = interaction else {
            self.interaction = interaction;
            return;
        };
        let DateEdit {
            list_id,
            task_id,
            editor,
        } = edit;
        let Some(etag) = self
            .board
            .task(&list_id, &task_id)
            .map(|task| task.etag.clone())
        else {
            self.recover("edited task disappeared".into());
            return;
        };
        let due = editor.date;
        self.error = None;
        let op = Op::Patch {
            list: list_id.clone(),
            task: task_id.clone(),
            patch: TaskPatch {
                due: Some(due),
                ..Default::default()
            },
            etag,
        };
        self.submit("save", op, &[&list_id], |board| {
            if let Some(task) = board.task_mut(&list_id, &task_id) {
                task.due = Some(due);
            }
        });
    }

    /// Edit the focused shelf field, opening the shelf on the title if it's shut.
    pub(super) fn start_edit(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(col) = self.columns.get(self.focus) else {
            return;
        };
        let Some(row) = col.selected() else { return };
        let field = match self.shelf {
            Shelf::Open(field) => field,
            Shelf::Closed => FieldId::Title,
        };
        if field == FieldId::Notes && row.task.assignment_info.is_some() {
            self.error = Some("tasks assigned from Docs or Chat cannot have notes".into());
            return;
        }
        let (list_id, task_id) = (col.list.id.clone(), row.task.id.clone());
        let buf = match field {
            FieldId::Title => row.task.title.clone(),
            FieldId::Notes => row.task.notes.clone().unwrap_or_default(),
        };
        self.shelf = Shelf::Open(field);
        self.interaction = Interaction::Editing(Edit {
            target: EditTarget::Existing {
                list_id,
                task_id,
                field,
            },
            cursor: buf.len(),
            buf,
        });
    }

    pub(super) fn toggle_completed(&mut self) {
        let selected = self.selected_task().map(|row| row.task.id.clone());
        self.show_completed = !self.show_completed;
        let ids: Vec<String> = self
            .columns
            .iter()
            .map(|column| column.list.id.clone())
            .collect();
        for id in ids {
            self.rebuild_column(&id);
        }
        let still_selected = self
            .selected_task()
            .is_some_and(|row| Some(row.task.id.as_str()) == selected.as_deref());
        if !still_selected {
            self.shelf = Shelf::Closed;
        }
    }

    pub(super) fn commit_edit(&mut self) {
        if !self.writable() {
            return;
        }
        let interaction = std::mem::replace(&mut self.interaction, Interaction::Idle);
        let Interaction::Editing(e) = interaction else {
            self.interaction = interaction;
            return;
        };
        if let EditTarget::New {
            list_id, parent, ..
        } = &e.target
        {
            let title = match normalize_title(&e.buf) {
                Ok(title) => title,
                Err(error) => {
                    self.error = Some(error.into());
                    self.interaction = Interaction::Editing(e);
                    return;
                }
            };
            self.error = None;
            let op = Op::Create {
                list: list_id.clone(),
                task: CreateTask {
                    title,
                    ..Default::default()
                },
                parent: parent.clone(),
            };
            self.submit_create(e, op);
            return;
        }
        let Some((list_id, task_id, field)) = e.existing() else {
            return;
        };
        let (list_id, task_id) = (list_id.to_string(), task_id.to_string());
        let Some(etag) = self
            .board
            .task(&list_id, &task_id)
            .map(|task| task.etag.clone())
        else {
            self.recover("edited task disappeared".into());
            return;
        };
        let patch = match field {
            FieldId::Title => match normalize_title(&e.buf) {
                Ok(title) => TaskPatch {
                    title: Some(title),
                    ..Default::default()
                },
                Err(error) => {
                    self.error = Some(error.into());
                    self.interaction = Interaction::Editing(e);
                    return;
                }
            },
            FieldId::Notes => TaskPatch {
                notes: Some(e.buf.trim_end().to_string()),
                ..Default::default()
            },
        };
        self.error = None;
        // Show the change straight away; Google confirms it a moment later.
        let (title, notes) = (patch.title.clone(), patch.notes.clone());
        let op = Op::Patch {
            list: list_id.clone(),
            task: task_id.clone(),
            patch,
            etag,
        };
        self.submit("save", op, &[&list_id], |board| {
            if let Some(task) = board.task_mut(&list_id, &task_id) {
                if let Some(title) = title {
                    task.title = title;
                }
                if let Some(notes) = notes {
                    task.notes = Some(notes).filter(|notes| !notes.is_empty());
                }
            }
        });
    }

    /// Tick or untick the selected task.
    pub(super) fn toggle_done(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(col) = self.columns.get(self.focus) else {
            return;
        };
        let list_id = col.list.id.clone();
        let Some(task_id) = col.selected_id.clone() else {
            return;
        };
        let Some(task) = self.board.task(&list_id, &task_id) else {
            return;
        };
        let status = if task.is_done() {
            Status::NeedsAction
        } else {
            Status::Completed
        };
        let op = Op::Patch {
            list: list_id.clone(),
            task: task_id.clone(),
            patch: TaskPatch::status(status),
            etag: task.etag.clone(),
        };
        // Show the new state now; Google's copy replaces it when it answers.
        self.submit("save", op, &[&list_id], |board| {
            if let Some(task) = board.task_mut(&list_id, &task_id) {
                task.status = status;
                if status == Status::NeedsAction {
                    task.completed = None;
                }
            }
        });
    }

    /// Ask before deleting; `confirm_delete` does the work after the configured
    /// confirmation key.
    pub(super) fn ask_delete(&mut self) {
        if !self.writable() {
            return;
        }
        let confirm = self.keymap.labels_for(Action::ConfirmDelete).join("/");
        let Some(col) = self.columns.get(self.focus) else {
            return;
        };
        let Some(row) = col.selected() else { return };
        let subtasks = self
            .board
            .rows(&col.list.id)
            .iter()
            .filter(|candidate| candidate.task.parent.as_deref() == Some(row.task.id.as_str()))
            .count();
        let also = match subtasks {
            0 => String::new(),
            1 => " and its subtask".into(),
            n => format!(" and its {n} subtasks"),
        };
        let assigned = if row.task.assignment_info.is_some() {
            " This also deletes the original Docs/Chat task."
        } else {
            ""
        };
        self.interaction = Interaction::ConfirmDelete(Pending {
            target: PendingTarget::Task {
                list_id: col.list.id.clone(),
                task_id: row.task.id.clone(),
                etag: row.task.etag.clone(),
            },
            prompt: format!(
                "Delete \"{}\"{also}?{assigned}  {confirm} confirms · any other key cancels",
                title_of(&row.task)
            ),
        });
    }

    /// Ask before deleting the focused list; Google removes every task in it.
    pub(super) fn ask_delete_list(&mut self) {
        if !self.writable() {
            return;
        }
        let confirm = self.keymap.labels_for(Action::ConfirmDelete).join("/");
        let Some(col) = self.columns.get(self.focus) else {
            return;
        };
        let count = col.rows.len();
        let tasks = match count {
            0 => " (empty)".into(),
            1 => " and its 1 task".into(),
            n => format!(" and its {n} tasks"),
        };
        let title = clean(&col.list.title).into_owned();
        self.interaction = Interaction::ConfirmDelete(Pending {
            target: PendingTarget::List {
                list_id: col.list.id.clone(),
            },
            prompt: format!(
                "Delete list \"{title}\"{tasks}?  {confirm} confirms · any other key cancels"
            ),
        });
    }

    /// Ask before clearing completed tasks from the focused list.
    pub(super) fn ask_clear_completed(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(col) = self.columns.get(self.focus) else {
            return;
        };
        let count = self
            .board
            .tasks(&col.list.id)
            .iter()
            .filter(|task| task.is_done())
            .count();
        if count == 0 {
            self.error = Some("no completed tasks to clear".into());
            return;
        }
        let confirm = self.keymap.labels_for(Action::ConfirmDelete).join("/");
        let title = clean(&col.list.title).into_owned();
        let tasks = match count {
            1 => "1 completed task".into(),
            n => format!("{n} completed tasks"),
        };
        self.interaction = Interaction::ConfirmDelete(Pending {
            target: PendingTarget::ClearCompleted {
                list_id: col.list.id.clone(),
            },
            prompt: format!(
                "Clear {tasks} from \"{title}\"?  {confirm} confirms · any other key cancels"
            ),
        });
    }

    pub(super) fn confirm_delete(&mut self) {
        let interaction = std::mem::replace(&mut self.interaction, Interaction::Idle);
        let Interaction::ConfirmDelete(pending) = interaction else {
            self.interaction = interaction;
            return;
        };
        self.error = None;
        match pending.target {
            PendingTarget::Task {
                list_id,
                task_id,
                etag,
            } => {
                let op = Op::Delete {
                    list: list_id.clone(),
                    task: task_id.clone(),
                    etag,
                };
                // Drop the task and its subtasks locally; Google cascades the same way.
                self.submit("delete", op, &[&list_id], |board| {
                    board.remove_tree(&list_id, &task_id);
                });
            }
            PendingTarget::List { list_id } => {
                let Some(index) = self.board.list_ids().iter().position(|id| id == &list_id) else {
                    return;
                };
                let Some(list) = self.board.list(&list_id).cloned() else {
                    return;
                };
                let tasks = self.board.tasks(&list_id);
                let backup = super::mutation::ListBackup { index, list, tasks };
                let op = Op::DeleteList {
                    list: list_id.clone(),
                };
                self.submit_list_delete("delete list", op, backup, |board| {
                    board.remove_list(&list_id);
                });
            }
            PendingTarget::ClearCompleted { list_id } => {
                let op = Op::ClearCompleted {
                    list: list_id.clone(),
                };
                self.submit("clear completed", op, &[&list_id], |board| {
                    board.clear_completed(&list_id);
                });
            }
        }
    }

    /// Open the floating list-title editor for the focused list.
    pub(super) fn start_rename_list(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(col) = self.columns.get(self.focus) else {
            return;
        };
        self.interaction = Interaction::ListEdit(ListEdit::rename(
            col.list.id.clone(),
            col.list.etag.clone(),
            col.list.title.clone(),
        ));
    }

    /// Open the floating panel to name a new list.
    pub(super) fn start_create_list(&mut self) {
        if !self.writable() {
            return;
        }
        self.interaction = Interaction::ListEdit(ListEdit::create());
    }

    /// Apply the list-title editor via `tasklists.patch` or `tasklists.insert`.
    pub(super) fn commit_list_edit(&mut self) {
        if !self.writable() {
            return;
        }
        let interaction = std::mem::replace(&mut self.interaction, Interaction::Idle);
        let Interaction::ListEdit(edit) = interaction else {
            self.interaction = interaction;
            return;
        };
        let title = match normalize_title(&edit.buf) {
            Ok(title) => title,
            Err(_) => {
                self.error = Some("a list needs a title".into());
                self.interaction = Interaction::ListEdit(edit);
                return;
            }
        };
        self.error = None;
        if edit.is_create() {
            let op = Op::CreateList {
                title: title.clone(),
            };
            self.submit_list_create(edit, op);
            return;
        }
        let Some(before) = self.board.list(&edit.list_id).cloned() else {
            self.recover("edited list disappeared".into());
            return;
        };
        let list_id = edit.list_id;
        let etag = edit.etag;
        let op = Op::PatchList {
            list: list_id.clone(),
            title: title.clone(),
            etag,
        };
        self.submit_list_rename("rename list", op, before, |board| {
            if let Some(list) = board.list_mut(&list_id) {
                list.title = title;
            }
        });
    }

    /// Move the focused list one place left or right in the local display order.
    /// Google has no tasklist move API, so this does not call the network.
    pub(super) fn reorder_list(&mut self, right: bool) {
        if !self.writable() {
            return;
        }
        let Some(list_id) = self.columns.get(self.focus).map(|c| c.list.id.clone()) else {
            return;
        };
        match self.board.move_list(&list_id, !right) {
            Ok(()) => {
                self.error = None;
                self.rebuild_columns_from_board(Some(&list_id));
            }
            Err(message) => {
                self.error = Some(message.into());
            }
        }
    }

    /// Copy the selected task as markdown, ready to paste into a note.
    pub(super) fn yank(&mut self) {
        let Some(row) = self.selected_task() else {
            return;
        };
        // The same control characters that would wreck a row would paste badly.
        let md = md_yank(&row.task);
        let title = title_of(&row.task).into_owned();
        match desktop::copy(&md) {
            Ok(Clipboard::Confirmed) => {
                self.error = None;
                self.notice = Some(format!("copied \"{title}\" as markdown"));
            }
            Ok(Clipboard::SequenceSent) => {
                self.error = None;
                self.notice = Some(format!("sent \"{title}\" to the terminal clipboard"));
            }
            Err(error) => {
                self.notice = None;
                self.error = Some(format!("clipboard failed: {error}"));
            }
        }
    }

    pub(super) fn start_browse(&mut self) {
        let Some(row) = self.selected_task() else {
            return;
        };
        let links = task_links(&row.task);
        if links.is_empty() {
            self.error = Some("no links on this task".into());
            return;
        }
        self.error = None;
        // A named link shows its URL underneath; a bare one is its own label.
        let rows = links
            .into_iter()
            .map(|link| match link.name {
                Some(name) => PickerRow::new(name, Some(link.url.clone()), link.url),
                None => PickerRow::new(link.url.clone(), None, link.url),
            })
            .collect();
        self.interaction = Interaction::Browse(Picker::new(rows));
    }

    /// Separator between list id and task id in a Search picker's opaque value.
    const SEARCH_SEP: char = '\x1f';

    /// Open a picker of every visible task across every list; `/` filter
    /// matches title and notes, subtitle is the list name, Enter focuses the
    /// match. Respects the completed-hidden toggle the same way the board does.
    pub(super) fn start_search(&mut self) {
        let mut rows = Vec::new();
        for list in self.board.lists() {
            let list_title = clean(&list.title).into_owned();
            for row in visible_rows(&self.board, &list.id, self.show_completed) {
                let notes = row.task.notes.as_deref().unwrap_or("");
                let value = format!("{}{}{}", list.id, Self::SEARCH_SEP, row.task.id);
                rows.push(
                    PickerRow::new(
                        title_of(&row.task).into_owned(),
                        Some(list_title.clone()),
                        value,
                    )
                    .filtering(notes),
                );
            }
        }
        if rows.is_empty() {
            self.error = Some("no tasks to search".into());
            return;
        }
        self.error = None;
        self.interaction = Interaction::Search(Picker::new(rows).with_filter_focus());
    }

    pub(super) fn confirm_search(&mut self) {
        let value = {
            let Interaction::Search(picker) = &self.interaction else {
                return;
            };
            let Some(row) = picker.selected() else {
                return;
            };
            row.value.clone()
        };
        self.interaction = Interaction::Idle;
        let Some((list_id, task_id)) = value.split_once(Self::SEARCH_SEP) else {
            return;
        };
        let Some(index) = self
            .columns
            .iter()
            .position(|column| column.list.id == list_id)
        else {
            return;
        };
        if self.board.task(list_id, task_id).is_none() {
            self.error = Some("task disappeared before it could be selected".into());
            return;
        }
        self.focus = index;
        self.columns[index].select_id(Some(task_id.into()));
    }

    pub(super) fn open_browse(&mut self, all: bool) {
        let urls: Vec<String> = {
            let Interaction::Browse(picker) = &self.interaction else {
                return;
            };
            match all {
                true => picker.values(),
                false => picker
                    .selected()
                    .map(|row| vec![row.value.clone()])
                    .unwrap_or_default(),
            }
        };
        if urls.is_empty() {
            return;
        }
        for url in &urls {
            if let Err(error) = desktop::open_url(url) {
                self.error = Some(format!("could not open {url}: {error}"));
                return;
            }
        }
        self.interaction = Interaction::Idle;
    }

    pub(super) fn start_parent_pick(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(column) = self.columns.get(self.focus) else {
            return;
        };
        let Some(selected) = column.selected() else {
            return;
        };
        let tree = self.board.rows(&column.list.id);
        if let Some(error) = subtask_block(&tree, &selected.task.id) {
            self.error = Some(error.into());
            return;
        }
        let tasks = eligible_parents(&tree, &selected.task.id);
        if tasks.is_empty() {
            self.error = Some("no available parent tasks in this list".into());
            return;
        }
        self.error = None;
        let rows = tasks
            .into_iter()
            .map(|task| PickerRow::new(title_of(&task).into_owned(), None, task.id))
            .collect();
        self.interaction = Interaction::Parent(ParentPick {
            list_id: column.list.id.clone(),
            task_id: selected.task.id.clone(),
            picker: Picker::new(rows),
        });
    }

    pub(super) fn confirm_parent(&mut self) {
        let selection = {
            let Interaction::Parent(parent) = &self.interaction else {
                return;
            };
            let Some(row) = parent.picker.selected() else {
                return;
            };
            (
                parent.list_id.clone(),
                parent.task_id.clone(),
                row.value.clone(),
            )
        };
        self.interaction = Interaction::Idle;
        self.reparent_task(&selection.0, &selection.1, Some(selection.2), None);
    }

    pub(super) fn unparent_task(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(column) = self.columns.get(self.focus) else {
            return;
        };
        let Some(selected) = column.selected() else {
            return;
        };
        if selected.task.parent.is_none() {
            self.error = Some("task is already top-level".into());
            return;
        }
        let list_id = column.list.id.clone();
        let task_id = selected.task.id.clone();
        let previous = selected.task.parent.clone();
        self.reparent_task(&list_id, &task_id, None, previous);
    }

    pub(super) fn reparent_task(
        &mut self,
        list_id: &str,
        task_id: &str,
        parent: Option<String>,
        previous: Option<String>,
    ) {
        if self.board.task(list_id, task_id).is_none() {
            self.error = Some("task disappeared before it could move".into());
            return;
        }
        self.error = None;
        let op = Op::Place {
            list: list_id.into(),
            task: task_id.into(),
            parent: parent.clone(),
            previous,
        };
        self.submit("reparent", op, &[list_id], |board| {
            if let Some(task) = board.task_mut(list_id, task_id) {
                task.parent = parent;
            }
        });
    }

    /// Move the selected task one place among its open siblings. Presses made
    /// while that task's reorder is saving move it locally and are sent as
    /// one move when the save lands.
    pub(super) fn reorder_task(&mut self, down: bool) {
        let Some(column) = self.columns.get(self.focus) else {
            return;
        };
        let Some(selected) = column.selected() else {
            return;
        };
        let list_id = column.list.id.clone();
        let task_id = selected.task.id.clone();
        let queue = self
            .pending_operation
            .as_ref()
            .and_then(|pending| pending.reorder.as_ref())
            .is_some_and(|reorder| reorder.list_id == list_id && reorder.task_id == task_id);
        if queue && self.sync != SyncState::Ready {
            self.error = Some(READ_ONLY.into());
            return;
        }
        if !queue && !self.writable() {
            return;
        }
        let plan = match reorder_step(&self.board.rows(&list_id), &task_id, down) {
            Ok(plan) => plan,
            Err(error) => {
                self.error = Some(error.into());
                return;
            }
        };
        self.error = None;
        if !queue {
            self.send_reorder(list_id, task_id, plan);
            return;
        }
        self.apply_positions(&list_id, &plan.positions);
        if let Some(reorder) = self
            .pending_operation
            .as_mut()
            .and_then(|pending| pending.reorder.as_mut())
        {
            reorder.queued = true;
        }
    }

    /// Send the presses made while a reorder was saving as one move to where
    /// the board showed the task.
    pub(super) fn send_queued_reorder(
        &mut self,
        list_id: String,
        task_id: String,
        previous: Option<String>,
    ) {
        if self.quit == QuitState::Now {
            return;
        }
        if self.sync != SyncState::Ready {
            self.error = Some("queued reorder dropped: the board is refreshing".into());
            return;
        }
        let rows = self.board.rows(&list_id);
        match reorder_after(&rows, &task_id, previous.as_deref()) {
            Ok(plan) if plan.positions.is_empty() => {}
            Ok(plan) => {
                if self.send_reorder(list_id, task_id, plan) && self.quit == QuitState::Drained {
                    // The queued move is a pending write, so a waiting quit keeps waiting.
                    self.quit = QuitState::Waiting;
                }
            }
            Err(error) => self.error = Some(format!("queued reorder dropped: {error}")),
        }
    }

    pub(super) fn send_reorder(
        &mut self,
        list_id: String,
        task_id: String,
        plan: ReorderPlan,
    ) -> bool {
        let op = Op::Place {
            list: list_id.clone(),
            task: task_id.clone(),
            parent: plan.parent,
            previous: plan.previous,
        };
        let positions = plan.positions;
        let sent = self.submit("reorder", op, &[&list_id], |board| {
            set_positions(board, &list_id, &positions)
        });
        if let Some(pending) = self.pending_operation.as_mut().filter(|_| sent) {
            pending.reorder = Some(Reorder {
                list_id,
                task_id,
                queued: false,
            });
        }
        sent
    }

    pub(super) fn apply_positions(&mut self, list_id: &str, positions: &[(String, String)]) {
        set_positions(&mut self.board, list_id, positions);
        self.rebuild_column(list_id);
    }

    pub(super) fn start_move(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(col) = self.columns.get(self.focus) else {
            return;
        };
        let Some(selected) = col.selected() else {
            return;
        };
        if let Some(error) = cross_list_block(&self.board.rows(&col.list.id), &selected.task.id) {
            self.error = Some(error.into());
            return;
        }
        let current = col.list.id.clone();
        let all: Vec<TaskList> = self.columns.iter().map(|c| c.list.clone()).collect();
        let lists: Vec<TaskList> = other_lists(&all, &current).into_iter().cloned().collect();
        if lists.is_empty() {
            self.error = Some("no other lists".into());
            return;
        }
        self.error = None;
        let rows = lists
            .into_iter()
            .map(|list| PickerRow::new(clean(&list.title).into_owned(), None, list.id))
            .collect();
        self.interaction = Interaction::Move(Picker::new(rows));
    }

    pub(super) fn confirm_move(&mut self) {
        let dest_id = {
            let Interaction::Move(picker) = &self.interaction else {
                return;
            };
            let Some(row) = picker.selected() else { return };
            row.value.clone()
        };
        self.interaction = Interaction::Idle;
        let Some(src) = self.columns.get(self.focus) else {
            return;
        };
        let from_id = src.list.id.clone();
        let Some(row) = src.selected() else { return };
        let task_id = row.task.id.clone();
        let ids = move_ids(&self.board.rows(&from_id), &task_id);
        if ids.is_empty() {
            return;
        }
        let Some(dest_idx) = self.columns.iter().position(|c| c.list.id == dest_id) else {
            return;
        };
        self.error = None;
        let op = Op::MoveToList {
            from: from_id.clone(),
            to: dest_id.clone(),
            ids,
        };
        let moved = self.submit("move", op, &[&from_id, &dest_id], |board| {
            board.move_tree(&from_id, &dest_id, &task_id);
        });
        if moved {
            self.columns[dest_idx].select_id(Some(task_id));
            self.focus = dest_idx;
        }
    }

    pub(super) fn start_help(&mut self) {
        let rows = Action::ALL
            .iter()
            .filter_map(|action| {
                let labels = self.keymap.labels_for(*action);
                (!labels.is_empty()).then(|| {
                    let key = labels.join(" / ");
                    let description = action.description();
                    PickerRow::new(key, Some(description.to_string()), String::new())
                })
            })
            .collect();
        self.interaction = Interaction::Help(Picker::reference(rows));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::test_support::*;
    use chrono::NaiveDate;

    #[test]
    fn d_opens_the_due_date_editor_and_enter_patches_the_task() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap();
        app.sync = SyncState::Ready;
        let mut open = task("a-one", None, "1", false);
        open.due = NaiveDate::from_ymd_opt(2026, 3, 9);
        set_tasks(&mut app, "a", vec![open], "a-one");

        app.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        let Interaction::DateEdit(edit) = &app.interaction else {
            panic!("expected date editor");
        };
        assert_eq!(
            edit.editor.date,
            NaiveDate::from_ymd_opt(2026, 3, 9).unwrap()
        );

        // Focus starts on day; bump once, then save.
        app.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(matches!(app.interaction, Interaction::Idle));
        assert_eq!(
            app.board.task("a", "a-one").and_then(|t| t.due),
            NaiveDate::from_ymd_opt(2026, 3, 10)
        );
        let (
            _,
            Op::Patch {
                list, task, patch, ..
            },
        ) = sent(&commands)
        else {
            panic!("expected due-date patch");
        };
        assert_eq!(list, "a");
        assert_eq!(task, "a-one");
        assert_eq!(patch.due, NaiveDate::from_ymd_opt(2026, 3, 10));
    }

    #[test]
    fn shift_d_confirms_and_clears_completed_tasks() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap();
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("open", None, "1", false),
                task("done-parent", None, "2", true),
                task("open-child", Some("done-parent"), "1", false),
                task("done", None, "3", true),
            ],
            "open",
        );

        app.on_key(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT));
        let Interaction::ConfirmDelete(pending) = &app.interaction else {
            panic!("expected clear confirm");
        };
        assert!(pending.prompt.contains("2 completed"));
        assert!(matches!(
            &pending.target,
            PendingTarget::ClearCompleted { list_id } if list_id == "a"
        ));

        app.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        let ids: Vec<String> = app
            .board
            .tasks("a")
            .into_iter()
            .map(|task| task.id)
            .collect();
        assert_eq!(ids, ["open".to_string(), "open-child".to_string()]);
        assert!(app
            .board
            .task("a", "open-child")
            .is_some_and(|task| task.parent.is_none()));
        let (operation, Op::ClearCompleted { list }) = sent(&commands) else {
            panic!("expected ClearCompleted");
        };
        assert_eq!(list, "a");

        app.on_msg(finished(
            operation,
            vec![(
                "a",
                vec![
                    task("open", None, "1", false),
                    task("open-child", None, "2", false),
                ],
            )],
        ));
        let ids: Vec<&str> = app
            .columns
            .iter()
            .find(|column| column.list.id == "a")
            .unwrap()
            .rows
            .iter()
            .map(|row| row.task.id.as_str())
            .collect();
        assert_eq!(ids, ["open", "open-child"]);
    }

    #[test]
    fn shift_d_with_nothing_completed_reports_and_sends_nothing() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap();
        app.sync = SyncState::Ready;
        set_tasks(&mut app, "a", vec![task("open", None, "1", false)], "open");

        app.on_key(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT));
        assert!(matches!(app.interaction, Interaction::Idle));
        assert!(app
            .error
            .as_deref()
            .is_some_and(|message| message.contains("no completed")));
        assert!(commands.try_recv().is_err());
    }

    #[test]
    fn failed_clear_completed_restores_the_list() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap();
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("open", None, "1", false),
                task("done", None, "2", true),
            ],
            "open",
        );

        app.on_key(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT));
        app.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        let (operation, _) = sent(&commands);
        app.on_msg(refused(operation));

        assert!(app.board.task("a", "done").is_some());
        assert!(app
            .error
            .as_deref()
            .is_some_and(|message| message.contains("clear completed")));
    }

    #[test]
    fn shift_x_confirms_and_deletes_the_focused_list() {
        let (mut app, commands) = app_with_lists(&["a", "b"]);
        let _ = commands.recv().unwrap();
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![task("a-one", None, "1", false)],
            "a-one",
        );

        app.on_key(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT));
        let Interaction::ConfirmDelete(pending) = &app.interaction else {
            panic!("expected list delete confirm");
        };
        assert!(pending.prompt.contains("Delete list"));
        assert!(matches!(
            &pending.target,
            PendingTarget::List { list_id } if list_id == "a"
        ));

        app.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert!(app.board.list("a").is_none());
        assert_eq!(
            app.columns
                .iter()
                .map(|column| column.list.id.as_str())
                .collect::<Vec<_>>(),
            ["b"]
        );
        let (_, Op::DeleteList { list }) = sent(&commands) else {
            panic!("expected DeleteList");
        };
        assert_eq!(list, "a");
    }

    #[test]
    fn shift_i_renames_the_focused_list() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap();
        app.sync = SyncState::Ready;

        app.on_key(KeyEvent::new(KeyCode::Char('I'), KeyModifiers::SHIFT));
        let Interaction::ListEdit(edit) = &app.interaction else {
            panic!("expected list title editor");
        };
        assert_eq!(edit.buf, "List a");

        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Idle));
        assert_eq!(
            app.board.list("a").map(|list| list.title.as_str()),
            Some("List au")
        );
        let (_, Op::PatchList { list, title, .. }) = sent(&commands) else {
            panic!("expected PatchList");
        };
        assert_eq!(list, "a");
        assert_eq!(title, "List au");
    }

    #[test]
    fn shift_t_creates_a_list_after_google_answers() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap();
        app.sync = SyncState::Ready;

        app.on_key(KeyEvent::new(KeyCode::Char('T'), KeyModifiers::SHIFT));
        let Interaction::ListEdit(edit) = &app.interaction else {
            panic!("expected new-list editor");
        };
        assert!(edit.is_create());
        assert!(edit.buf.is_empty());

        for c in "Inbox".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Idle));
        assert!(
            app.board.list("new").is_none(),
            "wait for Google before inserting"
        );
        let (operation, Op::CreateList { title }) = sent(&commands) else {
            panic!("expected CreateList");
        };
        assert_eq!(title, "Inbox");

        app.on_msg(Msg::Finished {
            id: operation,
            result: Ok(Outcome {
                upserted_lists: vec![TaskList {
                    id: "new".into(),
                    title: "Inbox".into(),
                    ..Default::default()
                }],
                ..Outcome::default()
            }),
        });
        assert!(app.board.list("new").is_some());
        assert_eq!(app.columns.last().map(|c| c.list.id.as_str()), Some("new"));
        assert_eq!(app.columns[app.focus].list.id, "new");
    }

    #[test]
    fn failed_list_create_reopens_the_draft() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap();
        app.sync = SyncState::Ready;

        app.on_key(KeyEvent::new(KeyCode::Char('T'), KeyModifiers::SHIFT));
        for c in "Draft".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let (operation, _) = sent(&commands);
        app.on_msg(refused(operation));

        let Interaction::ListEdit(edit) = &app.interaction else {
            panic!("expected draft reopened");
        };
        assert!(edit.is_create());
        assert_eq!(edit.buf, "Draft");
        assert!(app
            .error
            .as_deref()
            .is_some_and(|m| m.contains("create list")));
    }

    #[test]
    fn shift_h_and_l_reorder_lists_locally() {
        let (mut app, commands) = app_with_lists(&["a", "b", "c"]);
        let _ = commands.recv().unwrap();
        app.sync = SyncState::Ready;
        app.focus = 1; // list b

        app.on_key(KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT));
        assert_eq!(
            app.board.list_ids(),
            &["b".to_string(), "a".into(), "c".into()]
        );
        assert_eq!(app.columns[0].list.id, "b");
        assert!(commands.try_recv().is_err(), "list reorder must stay local");

        app.on_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT));
        assert_eq!(
            app.board.list_ids(),
            &["a".to_string(), "b".into(), "c".into()]
        );
        assert_eq!(app.columns[app.focus].list.id, "b");
    }

    #[test]
    fn failed_list_delete_restores_the_list() {
        let (mut app, commands) = app_with_lists(&["a", "b"]);
        let _ = commands.recv().unwrap();
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![task("a-one", None, "1", false)],
            "a-one",
        );

        app.on_key(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT));
        app.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        let (operation, _) = sent(&commands);
        app.on_msg(refused(operation));

        assert!(app.board.list("a").is_some());
        assert!(app.columns.iter().any(|column| column.list.id == "a"));
        assert!(app
            .error
            .as_deref()
            .is_some_and(|m| m.contains("delete list")));
    }

    #[test]
    fn hidden_completed_subtasks_stay_part_of_their_parent() {
        let (mut app, commands) = app_with_lists(&["a", "b"]);
        let _ = commands.recv().unwrap();
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("open", None, "1", false),
                task("done-child", Some("open"), "2", true),
                task("other", None, "3", false),
            ],
            "open",
        );
        set_tasks(
            &mut app,
            "b",
            vec![task("there", None, "1", false)],
            "there",
        );
        app.on_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));

        app.ask_delete();
        let Interaction::ConfirmDelete(pending) = &app.interaction else {
            panic!("expected delete confirmation");
        };
        assert!(
            pending.prompt.contains("subtask"),
            "hidden completed subtask should still be mentioned: {}",
            pending.prompt
        );
        app.interaction = Interaction::Idle;

        app.start_parent_pick();
        assert!(app
            .error
            .as_deref()
            .is_some_and(|error| error.contains("subtasks")));

        app.start_move();
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match sent(&commands) {
            (_, Op::MoveToList { ids, from, to }) => {
                assert_eq!(from, "a");
                assert_eq!(to, "b");
                assert_eq!(ids, ["open", "done-child"]);
            }
            _ => panic!("expected move"),
        }
    }

    #[test]
    fn completing_a_task_while_completed_are_hidden_keeps_the_save() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap();
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
        app.on_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

        assert_eq!(
            app.columns[0]
                .rows
                .iter()
                .map(|row| row.task.id.as_str())
                .collect::<Vec<_>>(),
            ["a-two"]
        );
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("a-two"));
        assert!(app.board.task("a", "a-one").unwrap().is_done());

        let (operation, _) = sent(&commands);
        let mut saved = task("a-one", None, "1", true);
        saved.title = "A one".into();
        app.on_msg(Msg::Finished {
            id: operation,
            result: Ok(Outcome {
                upserted: vec![("a".into(), saved)],
                ..Outcome::default()
            }),
        });

        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.sync, SyncState::Ready);
        assert_eq!(
            app.board.task("a", "a-one").map(|task| task.title.as_str()),
            Some("A one")
        );
        assert_eq!(
            app.columns[0]
                .rows
                .iter()
                .map(|row| row.task.id.as_str())
                .collect::<Vec<_>>(),
            ["a-two"]
        );
    }

    #[test]
    fn task_links_lists_attached_links_then_notes_urls() {
        use crate::model::Link;
        let link = |d: &str, u: &str| Link {
            description: d.into(),
            link: u.into(),
        };
        let task = Task {
            notes: Some("see https://notes.test/a and https://mail.test/#all/1".into()),
            links: vec![
                link("Check In For Your Visit", "https://mail.test/#all/1"),
                link("  ", "https://nameless.test"),
                link("same url again", "https://mail.test/#all/1"),
                link("no url at all", ""),
            ],
            ..Default::default()
        };
        let links = task_links(&task);
        let got: Vec<(Option<&str>, &str)> = links
            .iter()
            .map(|l| (l.name.as_deref(), l.url.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                // Google's own link keeps the source's title and comes first.
                (Some("Check In For Your Visit"), "https://mail.test/#all/1"),
                (None, "https://nameless.test"),
                // The same URL in the notes is not offered twice.
                (None, "https://notes.test/a"),
            ]
        );
        assert!(task_links(&Task::default()).is_empty());
    }

    #[test]
    fn a_yank_carries_the_links_and_drops_the_bookkeeping() {
        let task = Task {
            id: "t1".into(),
            title: "Check In For Yearly Checkup".into(),
            notes: Some("Need my vax\r\n".into()),
            status: Status::NeedsAction,
            due: chrono::NaiveDate::from_ymd_opt(2026, 9, 25),
            parent: Some("p9".into()),
            links: vec![crate::model::Link {
                description: "Check In For Your Upcoming Visit".into(),
                link: "https://mail.google.com/mail/#all/1a0".into(),
            }],
            ..Default::default()
        };
        let md = md_yank(&task);
        assert_eq!(
            md,
            "# Check In For Yearly Checkup\n\nDue: 2026-09-25\n\nNeed my vax\n\n\
             - [Check In For Your Upcoming Visit](<https://mail.google.com/mail/#all/1a0>)\n"
        );
        // The things that identify a task to the API have no place in a note.
        for noise in ["t1", "needsAction", "p9", "id:", "status:", "parent:"] {
            assert!(!md.contains(noise), "{noise:?} is still in {md:?}");
        }
    }

    #[test]
    fn other_lists_skip_the_current_one() {
        let lists = [
            TaskList {
                id: "a".into(),
                title: "Inbox".into(),
                ..Default::default()
            },
            TaskList {
                id: "b".into(),
                title: "Work".into(),
                ..Default::default()
            },
            TaskList {
                id: "c".into(),
                title: "Home".into(),
                ..Default::default()
            },
        ];
        let got: Vec<&str> = other_lists(&lists, "b")
            .iter()
            .map(|l| l.id.as_str())
            .collect();
        assert_eq!(got, ["a", "c"]);
        assert!(other_lists(&lists, "missing")
            .iter()
            .map(|l| l.id.as_str())
            .eq(["a", "b", "c"]));
    }

    #[test]
    fn assigned_tasks_cannot_move_or_edit_notes() {
        let (mut app, _cmds) = app_with_lists(&["a", "b"]);
        app.sync = SyncState::Ready;
        let assigned = Task {
            assignment_info: Some(crate::model::AssignmentInfo {
                link_to_task: "https://docs.google.com/document/d/1".into(),
                surface_type: "DOCUMENT".into(),
            }),
            ..task("assigned", None, "1", false)
        };
        set_tasks(&mut app, "a", vec![assigned], "assigned");

        app.start_move();
        assert!(matches!(app.interaction, Interaction::Idle));
        assert!(app
            .error
            .as_deref()
            .is_some_and(|error| error.contains("cannot move")));

        app.error = None;
        app.shelf = Shelf::Open(FieldId::Notes);
        app.start_edit();
        assert!(matches!(app.interaction, Interaction::Idle));
        assert!(app
            .error
            .as_deref()
            .is_some_and(|error| error.contains("cannot have notes")));
    }

    #[test]
    fn editing_after_closing_the_shelf_on_notes_starts_at_the_title() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        app.sync = SyncState::Ready;
        let assigned = Task {
            assignment_info: Some(crate::model::AssignmentInfo {
                link_to_task: "https://docs.google.com/document/d/1".into(),
                surface_type: "DOCUMENT".into(),
            }),
            ..task("assigned", None, "1", false)
        };
        set_tasks(&mut app, "a", vec![assigned], "assigned");

        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));

        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.edit().map(Edit::field), Some(FieldId::Title));
        assert_eq!(app.shelf, Shelf::Open(FieldId::Title));
    }

    #[test]
    fn less_than_unparents_the_selected_task() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("parent", None, "1", false),
                task("child", Some("parent"), "2", false),
            ],
            "child",
        );

        // Some enhanced terminals report the shifted symbol with no modifier.
        app.on_key(KeyEvent::new(KeyCode::Char('<'), KeyModifiers::NONE));

        assert_eq!(app.board.task("a", "child").unwrap().parent, None);
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("child"));
        assert!(matches!(
            commands.recv().unwrap(),
            Cmd::Run {
                op: Op::Place {
                list: list_id,
                task: task_id,
                parent: None,
                previous: Some(previous),
                ..
            },
                ..
            } if list_id == "a" && task_id == "child" && previous == "parent"
        ));
    }

    #[test]
    fn unparenting_keeps_its_place_after_google_renumbers_siblings() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("parent", None, "0", false),
                task("next", None, "1", false),
                task("child", Some("parent"), "0", false),
            ],
            "child",
        );

        app.unparent_task();
        let (id, _) = sent(&commands);
        // Google lands the child right after its former parent and bumps
        // every later sibling, so the child's new position equals `next`'s
        // old one.
        app.on_msg(finished(
            id,
            vec![(
                "a",
                vec![
                    task("parent", None, "0", false),
                    task("child", None, "1", false),
                    task("next", None, "2", false),
                ],
            )],
        ));

        assert_eq!(ids(&app, "a"), ["parent", "child", "next"]);
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("child"));
        assert!(app.error.is_none(), "{:?}", app.error);
    }

    #[test]
    fn alt_j_swaps_with_the_next_sibling_and_keeps_the_selection() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("first", None, "1", false),
                task("child", Some("first"), "1", false),
                task("second", None, "2", false),
                task("third", None, "3", false),
            ],
            "first",
        );

        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));

        assert_eq!(ids(&app, "a"), ["second", "first", "child", "third"]);
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("first"));
        let (
            operation,
            Op::Place {
                list: list_id,
                task: task_id,
                parent,
                previous,
                ..
            },
        ) = sent(&commands)
        else {
            panic!("expected a same-list move");
        };
        assert_eq!(
            (
                list_id.as_str(),
                task_id.as_str(),
                parent,
                previous.as_deref()
            ),
            ("a", "first", None, Some("second"))
        );

        // Google renumbers siblings as dense counters: the moved task takes
        // the neighbor's old slot and the neighbor is bumped. The reloaded
        // list replaces the local guess, so the row stays where it moved.
        app.on_msg(finished(
            operation,
            vec![(
                "a",
                vec![
                    task("second", None, "0", false),
                    task("first", None, "1", false),
                    task("child", Some("first"), "0", false),
                    task("third", None, "2", false),
                ],
            )],
        ));
        assert_eq!(app.board.task("a", "second").unwrap().position, "0");
        assert_eq!(app.board.task("a", "first").unwrap().position, "1");
        assert_eq!(ids(&app, "a"), ["second", "first", "child", "third"]);
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("first"));
        assert_eq!(app.sync, SyncState::Ready);
        assert!(app.pending_operation.is_none());
    }

    #[test]
    fn alt_k_moves_a_subtask_up_among_its_siblings() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("parent", None, "1", false),
                task("one", Some("parent"), "1", false),
                task("two", Some("parent"), "2", false),
            ],
            "two",
        );

        app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT));

        assert_eq!(ids(&app, "a"), ["parent", "two", "one"]);
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
            } if task_id == "two" && parent == "parent"
        ));
    }

    #[test]
    fn failed_reorder_restores_both_tasks_and_enters_recovery() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("first", None, "1", false),
                task("second", None, "2", false),
            ],
            "second",
        );

        app.reorder_task(false);
        assert_eq!(ids(&app, "a"), ["second", "first"]);
        let (operation, _) = sent(&commands);
        app.on_msg(refused(operation));

        assert_eq!(ids(&app, "a"), ["first", "second"]);
        assert_eq!(app.board.task("a", "first").unwrap().position, "1");
        assert_eq!(app.board.task("a", "second").unwrap().position, "2");
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("second"));
        assert_eq!(app.sync, SyncState::Refreshing);
        assert!(app
            .error
            .as_deref()
            .is_some_and(|error| error.contains("reorder failed")));
    }

    #[test]
    fn reorder_saved_but_not_reloaded_keeps_the_move_and_recovers() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("first", None, "0", false),
                task("second", None, "1", false),
            ],
            "second",
        );

        app.reorder_task(false);
        let (operation, _) = sent(&commands);
        app.on_msg(Msg::Finished {
            id: operation,
            result: Ok(Outcome {
                upserted: vec![("a".into(), task("second", None, "0", false))],
                warnings: vec![Warning::ReloadFailed(crate::google::Error::Timeout)],
                ..Outcome::default()
            }),
        });

        assert_eq!(ids(&app, "a"), ["second", "first"]);
        assert_eq!(app.sync, SyncState::Refreshing);
        assert!(app
            .error
            .as_deref()
            .is_some_and(|error| error.contains("reorder saved")));
    }

    #[test]
    fn reorder_at_an_edge_reports_and_sends_nothing() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("first", None, "1", false),
                task("second", None, "2", false),
            ],
            "first",
        );

        app.reorder_task(false);

        assert_eq!(app.error.as_deref(), Some("task is already first"));
        assert!(commands.try_recv().is_err());
        assert!(app.pending_operation.is_none());
    }

    #[test]
    fn reorder_presses_while_saving_become_one_follow_up_move() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        reorder_list(&mut app, &["a", "b", "c", "d"]);
        let down = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT);

        app.on_key(down);
        let (operation, Op::Place { previous, .. }) = sent(&commands) else {
            panic!("expected a same-list move");
        };
        assert_eq!(previous.as_deref(), Some("b"));

        app.on_key(down);
        app.on_key(down);
        assert!(commands.try_recv().is_err(), "presses wait for the save");
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(ids(&app, "a"), ["b", "c", "d", "a"]);

        app.on_msg(reordered(operation, &["b", "a", "c", "d"]));
        assert_eq!(ids(&app, "a"), ["b", "c", "d", "a"]);
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("a"));
        let (
            operation,
            Op::Place {
                task: task_id,
                previous,
                ..
            },
        ) = sent(&commands)
        else {
            panic!("expected the queued presses as one move");
        };
        assert_eq!((task_id.as_str(), previous.as_deref()), ("a", Some("d")));

        app.on_msg(reordered(operation, &["b", "c", "d", "a"]));
        assert!(commands.try_recv().is_err());
        assert_eq!(ids(&app, "a"), ["b", "c", "d", "a"]);
        assert!(app.pending_operation.is_none());
        assert_eq!(app.sync, SyncState::Ready);
    }

    #[test]
    fn queued_reorder_presses_that_cancel_out_still_land_where_shown() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        reorder_list(&mut app, &["a", "b", "c"]);

        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
        let (operation, _) = sent(&commands);
        app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT));
        assert_eq!(ids(&app, "a"), ["a", "b", "c"]);

        app.on_msg(reordered(operation, &["b", "a", "c"]));
        assert_eq!(ids(&app, "a"), ["a", "b", "c"]);
        assert!(matches!(
            commands.recv().unwrap(),
            Cmd::Run {
                op: Op::Place { task: task_id, previous: None, .. },
                ..
            } if task_id == "a"
        ));
    }

    #[test]
    fn a_failed_reorder_drops_queued_presses_and_restores_the_list() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        reorder_list(&mut app, &["a", "b", "c"]);

        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
        let (operation, _) = sent(&commands);
        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
        assert_eq!(ids(&app, "a"), ["b", "c", "a"]);

        app.on_msg(refused(operation));

        assert_eq!(ids(&app, "a"), ["a", "b", "c"]);
        assert!(matches!(commands.recv().unwrap(), Cmd::Refresh(_)));
        assert!(commands.try_recv().is_err());
        assert!(app.pending_operation.is_none());
    }

    #[test]
    fn a_refresh_during_a_reorder_drops_queued_presses_with_a_message() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        reorder_list(&mut app, &["a", "b", "c"]);

        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
        let (operation, _) = sent(&commands);
        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
        app.refresh();
        assert!(matches!(commands.recv().unwrap(), Cmd::Refresh(_)));

        app.on_msg(reordered(operation, &["b", "a", "c"]));

        assert!(commands.try_recv().is_err());
        assert_eq!(
            app.error.as_deref(),
            Some("queued reorder dropped: the board is refreshing")
        );
    }

    #[test]
    fn a_failed_reload_mentions_the_dropped_queued_presses() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        reorder_list(&mut app, &["a", "b", "c"]);

        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
        let (operation, _) = sent(&commands);
        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
        app.on_msg(Msg::Finished {
            id: operation,
            result: Ok(Outcome {
                upserted: vec![("a".into(), task("a", None, "1", false))],
                warnings: vec![Warning::ReloadFailed(crate::google::Error::Timeout)],
                ..Outcome::default()
            }),
        });

        assert!(matches!(commands.recv().unwrap(), Cmd::Refresh(_)));
        assert!(commands.try_recv().is_err());
        assert!(app
            .error
            .as_deref()
            .is_some_and(|error| error.contains("reorder saved")
                && error.ends_with("queued reorder dropped")));
    }

    #[test]
    fn failed_reparent_rolls_back_and_enters_recovery() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("parent", None, "1", false),
                task("child", Some("parent"), "2", false),
            ],
            "child",
        );

        app.unparent_task();
        let (operation, _) = sent(&commands);
        app.on_msg(refused(operation));

        assert_eq!(
            app.board
                .task("a", "child")
                .and_then(|task| task.parent.as_deref()),
            Some("parent")
        );
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("child"));
        assert_eq!(app.sync, SyncState::Refreshing);
        assert!(app
            .error
            .as_deref()
            .is_some_and(|error| error.contains("reparent failed")));
    }

    #[test]
    fn greater_than_lists_only_available_parents_and_reparents() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        let assigned = Task {
            assignment_info: Some(crate::model::AssignmentInfo {
                link_to_task: "https://docs.google.com/document/d/1".into(),
                surface_type: "DOCUMENT".into(),
            }),
            ..task("assigned", None, "5", false)
        };
        let hidden = Task {
            hidden: true,
            ..task("hidden", None, "6", false)
        };
        set_tasks(
            &mut app,
            "a",
            vec![
                task("selected", None, "1", false),
                task("available", None, "2", false),
                task("completed", None, "3", true),
                task("subtask", Some("available"), "4", false),
                assigned,
                hidden,
            ],
            "selected",
        );

        // Other terminals retain the base key and report Shift separately.
        app.on_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::SHIFT));
        let Interaction::Parent(picker) = &app.interaction else {
            panic!("expected parent picker");
        };
        assert_eq!(
            picker
                .picker
                .rows()
                .map(|row| row.value.as_str())
                .collect::<Vec<_>>(),
            ["available"]
        );

        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.board
                .task("a", "selected")
                .and_then(|task| task.parent.as_deref()),
            Some("available")
        );
        assert!(matches!(
            commands.recv().unwrap(),
            Cmd::Run {
                op: Op::Place {
                list: list_id,
                task: task_id,
                parent: Some(parent),
                ..
            },
                ..
            } if list_id == "a" && task_id == "selected" && parent == "available"
        ));
    }

    #[test]
    fn tasks_with_children_cannot_become_subtasks() {
        let (mut app, _commands) = app_with_lists(&["a"]);
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("has-children", None, "1", false),
                task("child", Some("has-children"), "2", false),
                // Cleared/hidden completed tasks are dropped on load; the
                // board-level guard for them is covered in board tests.
                Task {
                    hidden: true,
                    ..task("completed-hidden", None, "3", true)
                },
                task("available", None, "4", false),
            ],
            "has-children",
        );
        assert!(app.board.task("a", "completed-hidden").is_none());

        app.start_parent_pick();

        assert!(matches!(app.interaction, Interaction::Idle));
        assert!(app
            .error
            .as_deref()
            .is_some_and(|error| error.contains("cannot become a subtask")));
    }

    #[test]
    fn slash_searches_across_lists_and_enter_focuses_the_match() {
        let (mut app, commands) = app_with_lists(&["a", "b"]);
        let _ = commands.recv().unwrap();
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("a-one", None, "1", false),
                Task {
                    notes: Some("buy milk and eggs".into()),
                    ..task("a-shop", None, "2", false)
                },
            ],
            "a-one",
        );
        set_tasks(
            &mut app,
            "b",
            vec![task("b-write", None, "1", false)],
            "b-write",
        );
        app.focus = 0;

        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        let Interaction::Search(picker) = &app.interaction else {
            panic!("expected search picker");
        };
        assert!(picker.typing());
        assert_eq!(picker.rows().count(), 3);

        for c in "milk".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let Interaction::Search(picker) = &app.interaction else {
            panic!("expected search picker");
        };
        let values: Vec<String> = picker.rows().map(|row| row.value.clone()).collect();
        assert_eq!(values, [format!("a{}a-shop", App::SEARCH_SEP)]);
        assert_eq!(
            picker.selected().and_then(|row| row.subtitle.as_deref()),
            Some("List a")
        );

        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Idle));
        assert_eq!(app.focus, 0);
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("a-shop"));
    }

    #[test]
    fn search_omits_completed_tasks_when_they_are_hidden() {
        let (mut app, commands) = app_with_lists(&["a", "b"]);
        let _ = commands.recv().unwrap();
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                Task {
                    title: "open parent".into(),
                    ..task("parent", None, "1", true)
                },
                Task {
                    title: "open child".into(),
                    parent: Some("parent".into()),
                    ..task("child", None, "1", false)
                },
            ],
            "parent",
        );
        set_tasks(
            &mut app,
            "b",
            vec![
                Task {
                    title: "still open".into(),
                    ..task("open-b", None, "1", false)
                },
                Task {
                    title: "finished chore".into(),
                    ..task("done-b", None, "2", true)
                },
            ],
            "open-b",
        );
        app.show_completed = false;
        app.rebuild_column("a");
        app.rebuild_column("b");

        app.start_search();
        let Interaction::Search(picker) = &app.interaction else {
            panic!("expected search picker");
        };
        let values: Vec<_> = picker.rows().map(|row| row.value.clone()).collect();
        assert!(
            values.contains(&format!("a{}parent", App::SEARCH_SEP)),
            "completed parents with open children stay searchable: {values:?}"
        );
        assert!(
            values.contains(&format!("a{}child", App::SEARCH_SEP)),
            "open children stay searchable: {values:?}"
        );
        assert!(
            values.contains(&format!("b{}open-b", App::SEARCH_SEP)),
            "open tasks stay searchable: {values:?}"
        );
        assert!(
            !values.iter().any(|value| value.contains("done-b")),
            "hidden completed tasks must not appear: {values:?}"
        );
        assert_eq!(picker.rows().count(), 3);
    }

    #[test]
    fn help_keys_lists_the_board_commands() {
        let (mut app, _commands) = app_with_lists(&["a"]);
        app.start_help();
        let picker = app.interaction.picker().unwrap();
        for need in [
            "previous list",
            "next task",
            "new task",
            "new subtask",
            "delete task",
            "search tasks",
            "move to another list",
            "choose parent",
            "show / hide completed",
            "keyboard commands",
            "quit",
        ] {
            assert!(
                picker
                    .rows()
                    .any(|row| row.subtitle.as_deref() == Some(need)),
                "missing {need}"
            );
        }
        assert!(picker
            .rows()
            .all(|row| !row.title.is_empty() && row.subtitle.is_some()));
    }
}
