use std::{
    borrow::Cow,
    io,
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use chrono::{DateTime, Datelike, Local, NaiveDate};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use ratatui::{
    crossterm::{
        event::{
            self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
            PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
        },
        execute, terminal,
    },
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Clear, HighlightSpacing, List, ListItem, ListState, Padding,
        Paragraph,
    },
    DefaultTerminal, Frame,
};

use crate::api::{
    Client, CreateTask, MoveTask, Task, TaskList, TaskPatch, TasksApi,
};
use crate::auth;
use crate::board::{Board, Row};
#[cfg(test)]
use crate::board::ordered as order;
use crate::cache;
use crate::config::{self, Config};
use crate::paths::Paths;
use crate::platform::{self, Clipboard};
use crate::presentation::{md_yank, normalize_title, urls_in};
use crate::theme::{self, Theme};

const THEME_POLL: Duration = Duration::from_millis(500);
const TICK: Duration = Duration::from_millis(100);
const COL_MIN: u16 = 30;
const COL_MAX: u16 = 56;
const SHELF_W: u16 = 52;
const READ_ONLY: &str = "Syncing with Google — read-only for a moment";

/// Requests to the network thread.
enum Cmd {
    Refresh(u64),
    Patch {
        operation: u64,
        list_id: String,
        task_id: String,
        patch: TaskPatch,
        etag: String,
    },
    Delete {
        operation: u64,
        list_id: String,
        task_id: String,
        etag: String,
    },
    Create {
        operation: u64,
        list_id: String,
        task: CreateTask,
        parent: Option<String>,
        selection: Option<String>,
    },
    Move {
        operation: u64,
        from: String,
        to: String,
        ids: Vec<String>,
    },
}

/// Results from the network thread.
enum Msg {
    Lists(u64, Result<Vec<TaskList>, String>),
    Tasks(u64, String, Result<Vec<Task>, String>),
    Updated {
        operation: u64,
        list_id: String,
        task_id: String,
        result: Result<Task, String>,
    },
    Deleted {
        operation: u64,
        list_id: String,
        task_id: String,
        result: Result<(), String>,
    },
    Created {
        operation: u64,
        list_id: String,
        selection: Option<String>,
        result: Result<Task, String>,
    },
    Moved {
        operation: u64,
        from: String,
        to: String,
        ids: Vec<String>,
        result: MoveOutcome,
    },
}

struct MoveFailure {
    task_id: String,
    error: String,
}

struct MoveOutcome {
    moved: Vec<Task>,
    failure: Option<MoveFailure>,
    source: Result<Vec<Task>, String>,
    destination: Result<Vec<Task>, String>,
}

fn move_requests(destination: &str, ids: &[String]) -> Vec<(String, MoveTask)> {
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

/// A delete waiting on a y/n answer in the status bar.
struct Pending {
    list_id: String,
    task_id: String,
    etag: String,
    prompt: String,
}

/// One row of the Browse overlay.
struct BrowseLink {
    /// The source's title, for links Google attached to the task. URLs picked
    /// out of the notes have nothing to call themselves.
    name: Option<String>,
    url: String,
}

/// Overlay listing everything the selected task links to.
struct Browse {
    links: Vec<BrowseLink>,
    state: ListState,
}

/// Overlay listing destination lists for a move.
struct MovePick {
    lists: Vec<TaskList>,
    state: ListState,
}

/// The editable fields of the shelf.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldId {
    Title,
    Notes,
}

enum EditTarget {
    Existing {
        list_id: String,
        task_id: String,
        field: FieldId,
    },
    New {
        list_id: String,
        parent: Option<String>,
        selection: Option<String>,
    },
}

/// An in-progress edit. `cursor` is a byte offset into `buf`.
struct Edit {
    target: EditTarget,
    buf: String,
    cursor: usize,
}

impl Edit {
    fn field(&self) -> FieldId {
        match self.target {
            EditTarget::Existing { field, .. } => field,
            EditTarget::New { .. } => FieldId::Title,
        }
    }

    fn existing(&self) -> Option<(&str, &str, FieldId)> {
        match &self.target {
            EditTarget::Existing {
                list_id,
                task_id,
                field,
            } => Some((list_id, task_id, *field)),
            EditTarget::New { .. } => None,
        }
    }

    fn is_new(&self) -> bool {
        matches!(self.target, EditTarget::New { .. })
    }

    fn insert(&mut self, c: char) {
        self.buf.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    fn left(&mut self) {
        if let Some((i, _)) = self.buf[..self.cursor].grapheme_indices(true).next_back() {
            self.cursor = i;
        }
    }

    fn right(&mut self) {
        if let Some(grapheme) = self.buf[self.cursor..].graphemes(true).next() {
            self.cursor += grapheme.len();
        }
    }

    fn backspace(&mut self) {
        let end = self.cursor;
        self.left();
        self.buf.replace_range(self.cursor..end, "");
    }

    fn delete(&mut self) {
        if let Some(grapheme) = self.buf[self.cursor..].graphemes(true).next() {
            let end = self.cursor + grapheme.len();
            self.buf.replace_range(self.cursor..end, "");
        }
    }

    fn kill_word(&mut self) {
        let head = self.buf[..self.cursor].trim_end_matches(char::is_whitespace);
        let start = head
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map_or(0, |(i, c)| i + c.len_utf8());
        self.buf.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    fn kill_to_start(&mut self) {
        let start = self.line_start();
        self.buf.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    fn line_start(&self) -> usize {
        self.buf[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.buf[self.cursor..].find('\n').map_or(self.buf.len(), |i| self.cursor + i)
    }

    fn step_visual_line(&mut self, width: usize, down: bool) {
        let segments = wrap(&self.buf, width);
        let (row, column) = cursor_rc(&segments, &self.buf, self.cursor);
        let target = if down {
            (row + 1).min(segments.len().saturating_sub(1))
        } else {
            row.saturating_sub(1)
        };
        if target == row {
            return;
        }
        let (start, end) = segments[target];
        self.cursor = byte_at_column(&self.buf, start, end, column);
    }

    /// The slice that fits `width` columns with the cursor in view, and the
    /// cursor's offset within it.
    fn view(&self, width: usize) -> (&str, usize) {
        let width = width.max(1);
        let mut boundaries: Vec<usize> =
            self.buf.grapheme_indices(true).map(|(index, _)| index).collect();
        boundaries.push(self.buf.len());
        let current = boundaries
            .iter()
            .position(|index| *index >= self.cursor)
            .unwrap_or(boundaries.len() - 1);
        let mut start = current;
        while start > 0
            && UnicodeWidthStr::width(&self.buf[boundaries[start - 1]..self.cursor]) < width
        {
            start -= 1;
        }
        let mut end = current;
        while end + 1 < boundaries.len()
            && UnicodeWidthStr::width(&self.buf[boundaries[start]..boundaries[end + 1]]) <= width
        {
            end += 1;
        }
        let from = boundaries[start];
        let to = boundaries[end];
        (
            &self.buf[from..to],
            UnicodeWidthStr::width(&self.buf[from..self.cursor]),
        )
    }
}

enum Interaction {
    Idle,
    Editing(Edit),
    ConfirmDelete(Pending),
    Browse(Browse),
    Move(MovePick),
    Help,
}

enum MutationContext {
    Create(Edit),
    Patch {
        list_id: String,
        task_id: String,
        before: Box<Task>,
    },
    Delete {
        list_id: String,
        task_id: String,
        removed: Vec<Task>,
    },
    Move {
        from: String,
        to: String,
        ids: Vec<String>,
        source_before: Vec<Task>,
        destination_before: Vec<Task>,
    },
}

struct PendingOperation {
    id: u64,
    context: MutationContext,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadState {
    Cached,
    Loading,
    Loaded,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SyncState {
    Cached,
    Refreshing,
    Ready,
    Failed,
}

struct Column {
    list: TaskList,
    rows: Vec<Row>,
    load: LoadState,
    selected_id: Option<String>,
    state: ListState,
}

impl Column {
    fn selected(&self) -> Option<&Row> {
        let selected = self.selected_id.as_deref()?;
        self.rows.iter().find(|row| row.task.id == selected)
    }

    fn select(&mut self, index: Option<usize>) {
        self.selected_id = index
            .and_then(|index| self.rows.get(index))
            .map(|row| row.task.id.clone());
        self.state.select(index);
    }

    fn select_id(&mut self, task_id: Option<String>) {
        let index = task_id
            .as_deref()
            .and_then(|id| self.rows.iter().position(|row| row.task.id == id));
        self.select(index);
    }
}

struct App {
    config: Config,
    theme: Theme,
    fingerprint: theme::Fingerprint,
    transparent: bool,
    board: Board,
    paths: Paths,
    cache_meta: cache::Meta,
    columns: Vec<Column>,
    focus: usize,
    /// Index of the leftmost visible column.
    scroll: usize,
    shelf: bool,
    /// Which shelf field the keyboard is on.
    field: FieldId,
    edit_width: usize,
    interaction: Interaction,
    /// Identifies the only refresh whose responses may update the board.
    refresh_generation: u64,
    sync: SyncState,
    next_operation: u64,
    pending_operation: Option<PendingOperation>,
    error: Option<String>,
    /// A one-off confirmation, cleared by the next keypress.
    notice: Option<String>,
    cmds: Sender<Cmd>,
    /// The user asked to quit and writes are still outstanding.
    quitting: bool,
    force_quit: bool,
    quit: bool,
}

impl App {
    #[cfg(test)]
    fn new(cmds: Sender<Cmd>, cached: cache::Cache, config: Config) -> Self {
        Self::new_with_storage(
            cmds,
            cached,
            config,
            Paths::from_env(),
            cache::Meta {
                account: String::new(),
                generation: 0,
            },
            None,
        )
    }

    fn new_with_storage(
        cmds: Sender<Cmd>,
        cached: cache::Cache,
        config: Config,
        paths: Paths,
        cache_meta: cache::Meta,
        warning: Option<String>,
    ) -> Self {
        // Paint the last known board straight away; the sync replaces it.
        let board = Board::from_cache(cached);
        let columns = board
            .lists()
            .map(|list| {
                let rows = board.rows(&list.id);
                let selected_id = rows.first().map(|row| row.task.id.clone());
                Column {
                list: list.clone(),
                state: ListState::default().with_selected((!rows.is_empty()).then_some(0)),
                rows,
                load: LoadState::Cached,
                selected_id,
            }
            })
            .collect();
        let mut app = Self {
            config,
            theme: theme::load(),
            fingerprint: theme::fingerprint(),
            transparent: false,
            board,
            paths,
            cache_meta,
            columns,
            focus: 0,
            scroll: 0,
            shelf: false,
            field: FieldId::Title,
            edit_width: 0,
            interaction: Interaction::Idle,
            refresh_generation: 0,
            sync: SyncState::Cached,
            next_operation: 0,
            pending_operation: None,
            error: warning,
            notice: None,
            cmds,
            quitting: false,
            force_quit: false,
            quit: false,
        };
        app.refresh();
        app
    }

    fn edit(&self) -> Option<&Edit> {
        match &self.interaction {
            Interaction::Editing(edit) => Some(edit),
            _ => None,
        }
    }

    fn edit_mut(&mut self) -> Option<&mut Edit> {
        match &mut self.interaction {
            Interaction::Editing(edit) => Some(edit),
            _ => None,
        }
    }

    fn is_creating(&self) -> bool {
        self.edit().is_some_and(Edit::is_new)
    }

    fn refresh(&mut self) {
        self.begin_refresh(None);
    }

    fn begin_refresh(&mut self, error: Option<String>) {
        self.error = error;
        self.sync = SyncState::Refreshing;
        if !self.cache_meta.account.is_empty() {
            if let Ok((_, meta)) = cache::load_for(&self.paths, &self.cache_meta.account) {
                self.cache_meta = meta;
            }
        }
        for col in &mut self.columns {
            col.load = LoadState::Cached;
        }
        self.refresh_generation = self.refresh_generation.wrapping_add(1);
        if self
            .cmds
            .send(Cmd::Refresh(self.refresh_generation))
            .is_err()
        {
            self.sync = SyncState::Failed;
            self.error = Some(match self.error.take() {
                Some(error) => format!("{error}; network worker stopped"),
                None => "network worker stopped".into(),
            });
        }
    }

    fn recover(&mut self, error: String) {
        if self.quitting {
            self.error = Some(error);
            self.sync = SyncState::Failed;
        } else {
            self.begin_refresh(Some(error));
        }
    }

    fn request_quit(&mut self) {
        if self.quitting {
            self.force_quit = true;
            self.quit = true;
        } else if self.pending_operation.is_some() {
            self.quitting = true;
            self.notice = Some("waiting for pending save · q again to force quit".into());
        } else {
            self.quit = true;
        }
    }

    fn mutation_finished(&mut self, operation: u64) -> Option<MutationContext> {
        if self.pending_operation.as_ref().map(|pending| pending.id) != Some(operation) {
            return None;
        }
        let context = self.pending_operation.take().map(|pending| pending.context);
        if self.quitting {
            self.quit = true;
        }
        context
    }

    fn send_mutation(
        &mut self,
        context: MutationContext,
        build: impl FnOnce(u64) -> Cmd,
    ) -> bool {
        if self.pending_operation.is_some() {
            self.error = Some("wait for the current save to finish".into());
            return false;
        }
        self.next_operation = self.next_operation.wrapping_add(1);
        let operation = self.next_operation;
        let cmd = build(operation);
        match self.cmds.send(cmd) {
            Ok(()) => {
                self.pending_operation = Some(PendingOperation {
                    id: operation,
                    context,
                });
                true
            }
            Err(_) => {
                self.rollback(context);
                self.sync = SyncState::Failed;
                self.error = Some("network worker stopped; change was not sent".into());
                false
            }
        }
    }

    fn rollback(&mut self, context: MutationContext) {
        match context {
            MutationContext::Create(edit) => {
                self.interaction = Interaction::Editing(edit);
            }
            MutationContext::Patch {
                list_id, before, ..
            } => {
                self.board.upsert_task(&list_id, *before);
                self.rebuild_column(&list_id);
            }
            MutationContext::Delete {
                list_id, removed, ..
            } => {
                self.board.restore(&list_id, removed);
                self.rebuild_column(&list_id);
            }
            MutationContext::Move {
                from,
                to,
                source_before,
                destination_before,
                ..
            } => {
                self.board.replace_tasks(&from, source_before);
                self.board.replace_tasks(&to, destination_before);
                self.rebuild_column(&from);
                self.rebuild_column(&to);
            }
        }
    }

    fn worker_disconnected(&mut self) {
        self.sync = SyncState::Failed;
        self.pending_operation = None;
        self.error = Some("network worker stopped".into());
        if self.quitting {
            self.quit = true;
        }
    }

    /// Editing waits until this run has seen Google's copy of every list.
    fn writable(&mut self) -> bool {
        if self.sync != SyncState::Ready {
            self.error = Some(READ_ONLY.into());
            return false;
        }
        if self.pending_operation.is_some() {
            self.error = Some("wait for the current save to finish".into());
            return false;
        }
        true
    }

    fn check_synced(&mut self) {
        if self.columns.iter().any(|c| c.load == LoadState::Failed) {
            self.sync = SyncState::Failed;
            return;
        }
        if !self.columns.iter().all(|c| c.load == LoadState::Loaded) {
            return;
        }
        self.sync = SyncState::Ready;
        if self.error.as_deref() == Some(READ_ONLY) {
            self.error = None;
        }
        self.save_cache();
    }

    fn save_cache(&mut self) -> bool {
        if self.sync != SyncState::Ready || self.pending_operation.is_some() {
            return false;
        }
        match cache::save_for(
            &self.paths,
            &self.board.snapshot(),
            &mut self.cache_meta,
        ) {
            Ok(()) => true,
            Err(error) => {
                self.error = Some(error.to_string());
                false
            }
        }
    }

    fn loading(&self) -> bool {
        self.sync == SyncState::Refreshing
    }

    /// Start a new task in the focused list: an empty title input in the shelf.
    /// `under` makes it a subtask of the selection (see `subtask_parent`).
    fn start_create(&mut self, under: bool) {
        if !self.writable() {
            return;
        }
        let Some(col) = self.columns.get(self.focus) else { return };
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
        self.shelf = true;
        self.field = FieldId::Title;
    }

    /// Edit the focused shelf field, opening the shelf on the title if it's shut.
    fn start_edit(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(col) = self.columns.get(self.focus) else { return };
        let Some(row) = col.selected() else { return };
        if self.field == FieldId::Notes && row.task.assignment_info.is_some() {
            self.error = Some("tasks assigned from Docs or Chat cannot have notes".into());
            return;
        }
        let (list_id, task_id) = (col.list.id.clone(), row.task.id.clone());
        let (title, notes) = (row.task.title.clone(), row.task.notes.clone().unwrap_or_default());
        if !self.shelf {
            self.shelf = true;
            self.field = FieldId::Title;
        }
        let buf = match self.field {
            FieldId::Title => title,
            FieldId::Notes => notes,
        };
        self.interaction = Interaction::Editing(Edit {
            target: EditTarget::Existing {
                list_id,
                task_id,
                field: self.field,
            },
            cursor: buf.len(),
            buf,
        });
    }

    fn rebuild_column(&mut self, list_id: &str) {
        let rows = self.board.rows(list_id);
        let Some(column) = self.columns.iter_mut().find(|column| column.list.id == list_id) else {
            return;
        };
        let selected = column.selected_id.clone();
        let previous = column.state.selected().unwrap_or(0);
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

    fn commit_edit(&mut self) {
        if !self.writable() {
            return;
        }
        let interaction = std::mem::replace(&mut self.interaction, Interaction::Idle);
        let Interaction::Editing(e) = interaction else {
            self.interaction = interaction;
            return;
        };
        if let EditTarget::New {
            list_id,
            parent,
            selection,
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
            let (list_id, parent, selection) =
                (list_id.clone(), parent.clone(), selection.clone());
            self.send_mutation(
                MutationContext::Create(e),
                |operation| Cmd::Create {
                    operation,
                    list_id,
                    task: CreateTask {
                        title,
                        ..Default::default()
                    },
                    parent,
                    selection,
                },
            );
            return;
        }
        // Show the change straight away; Google confirms it a moment later.
        let Some((list_id, task_id, field)) = e.existing() else {
            return;
        };
        let (list_id, task_id) = (list_id.to_string(), task_id.to_string());
        let Some(before) = self.board.task(&list_id, &task_id).cloned() else {
            self.recover("edited task disappeared".into());
            return;
        };
        let fields = match field {
            FieldId::Title => {
                let title = match normalize_title(&e.buf) {
                    Ok(title) => title,
                    Err(error) => {
                        self.error = Some(error.into());
                        self.interaction = Interaction::Editing(e);
                        return;
                    }
                };
                if let Some(task) = self.board.task_mut(&list_id, &task_id) {
                    task.title = title.clone();
                }
                self.rebuild_column(&list_id);
                TaskPatch {
                    title: Some(title),
                    ..Default::default()
                }
            }
            FieldId::Notes => {
                let notes = e.buf.trim_end().to_string();
                if let Some(task) = self.board.task_mut(&list_id, &task_id) {
                    task.notes = Some(notes.clone()).filter(|notes| !notes.is_empty());
                }
                self.rebuild_column(&list_id);
                TaskPatch {
                    notes: Some(notes),
                    ..Default::default()
                }
            }
        };
        self.error = None;
        self.send_patch(list_id, task_id, fields, before);
    }

    /// Tick or untick the selected task.
    fn toggle_done(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(col) = self.columns.get(self.focus) else { return };
        let list_id = col.list.id.clone();
        let Some(task_id) = col.selected_id.clone() else { return };
        let Some(task) = self.board.task_mut(&list_id, &task_id) else { return };
        let before = task.clone();
        // Show the new state now; Google's copy replaces it when it answers.
        let fields = if task.is_done() {
            task.status = "needsAction".into();
            task.completed = None;
            TaskPatch {
                status: Some("needsAction".into()),
                completed: Some(None),
                ..Default::default()
            }
        } else {
            task.status = "completed".into();
            TaskPatch {
                status: Some("completed".into()),
                ..Default::default()
            }
        };
        self.rebuild_column(&list_id);
        self.send_patch(list_id, task_id, fields, before);
    }

    /// Ask before deleting; `confirm_delete` does the work on "y".
    fn ask_delete(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(col) = self.columns.get(self.focus) else { return };
        let Some(row) = col.selected() else { return };
        let subtasks = col
            .rows
            .iter()
            .filter(|r| r.task.parent.as_deref() == Some(row.task.id.as_str()))
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
            list_id: col.list.id.clone(),
            task_id: row.task.id.clone(),
            etag: row.task.etag.clone(),
            prompt: format!("Delete \"{}\"{also}?{assigned}  y/n", title_of(&row.task)),
        });
    }

    fn confirm_delete(&mut self) {
        let interaction = std::mem::replace(&mut self.interaction, Interaction::Idle);
        let Interaction::ConfirmDelete(p) = interaction else {
            self.interaction = interaction;
            return;
        };
        // Drop the task and its subtasks locally; Google cascades the same way.
        let removed = self.board.remove_tree(&p.list_id, &p.task_id);
        self.rebuild_column(&p.list_id);
        self.error = None;
        let (list_id, task_id, etag) = (p.list_id, p.task_id, p.etag);
        self.send_mutation(
            MutationContext::Delete {
                list_id: list_id.clone(),
                task_id: task_id.clone(),
                removed,
            },
            |operation| Cmd::Delete {
                operation,
                list_id,
                task_id,
                etag,
            },
        );
    }

    fn send_patch(
        &mut self,
        list_id: String,
        task_id: String,
        patch: TaskPatch,
        before: Task,
    ) {
        let etag = before.etag.clone();
        self.send_mutation(
            MutationContext::Patch {
                list_id: list_id.clone(),
                task_id: task_id.clone(),
                before: Box::new(before),
            },
            |operation| Cmd::Patch {
                operation,
                list_id,
                task_id,
                patch,
                etag,
            },
        );
    }

    /// Reload the palette only when the fingerprint changes (cheap stat calls).
    fn check_theme(&mut self) {
        let fp = theme::fingerprint();
        if fp != self.fingerprint {
            self.fingerprint = fp;
            self.theme = theme::load();
        }
    }

    fn bg(&self, c: Color) -> Color {
        if self.transparent { Color::Reset } else { c }
    }

    fn on_msg(&mut self, msg: Msg) {
        match &msg {
            Msg::Lists(generation, _) | Msg::Tasks(generation, _, _)
                if *generation != self.refresh_generation =>
            {
                return;
            }
            _ => {}
        }
        match msg {
            Msg::Lists(_, Err(e)) => {
                self.sync = SyncState::Failed;
                self.error = Some(e);
            }
            Msg::Lists(_, Ok(lists)) => {
                // Keep existing columns (and their selection) until fresh tasks arrive.
                let focused = self.columns.get(self.focus).map(|c| c.list.id.clone());
                let mut old = std::mem::take(&mut self.columns);
                self.board.replace_lists(lists.clone());
                self.columns = lists
                    .into_iter()
                    .map(|list| {
                        let rows = self.board.rows(&list.id);
                        let mut column = match old.iter().position(|column| column.list.id == list.id)
                        {
                            Some(index) => old.swap_remove(index),
                            None => Column {
                                list: list.clone(),
                                rows: Vec::new(),
                                load: LoadState::Loading,
                                selected_id: None,
                                state: ListState::default(),
                            },
                        };
                        let selected = column.selected_id.clone();
                        column.list = list;
                        column.rows = rows;
                        column.load = LoadState::Loading;
                        column.select_id(
                            selected.or_else(|| {
                                column.rows.first().map(|row| row.task.id.clone())
                            }),
                        );
                        column
                    })
                    .collect();
                self.focus = focused
                    .and_then(|id| self.columns.iter().position(|c| c.list.id == id))
                    .unwrap_or(0);
                self.check_synced(); // an account with no lists is already in sync
            }
            Msg::Tasks(_, id, res) => {
                let Some(col) = self.columns.iter_mut().find(|c| c.list.id == id) else { return };
                match res {
                    Ok(tasks) => {
                        self.board.replace_tasks(&id, tasks);
                        col.load = LoadState::Loaded;
                        let keep = col.selected_id.clone();
                        let prev = col.state.selected();
                        col.rows = self.board.rows(&id);
                        let last = col.rows.len().checked_sub(1);
                        let sel = keep
                            .and_then(|id| col.rows.iter().position(|r| r.task.id == id))
                            .or_else(|| last.map(|l| prev.unwrap_or(0).min(l)));
                        col.select(sel);
                    }
                    Err(e) => {
                        col.load = LoadState::Failed;
                        self.error = Some(format!("{}: {e}", col.list.title));
                    }
                }
                self.check_synced();
            }
            Msg::Created {
                operation,
                list_id,
                selection: selection_at_send,
                result,
            } => {
                let Some(context) = self.mutation_finished(operation) else {
                    return;
                };
                let MutationContext::Create(draft) = context else {
                    self.recover("create response did not match the pending operation".into());
                    return;
                };
                let EditTarget::New {
                    list_id: draft_list,
                    selection: draft_selection,
                    ..
                } = &draft.target
                else {
                    self.recover("create response did not match its draft".into());
                    return;
                };
                if draft_list != &list_id || draft_selection != &selection_at_send {
                    self.recover("create response did not match its draft".into());
                    return;
                }
                match result {
                    Ok(task) => {
                        let focused_list =
                            self.columns.get(self.focus).map(|c| c.list.id.as_str());
                        let may_follow = self.edit().is_none()
                            && focused_list == Some(list_id.as_str());
                        let Some(col_index) =
                            self.columns.iter().position(|c| c.list.id == list_id)
                        else {
                            self.recover("create succeeded but its list disappeared".into());
                            return;
                        };
                        let id = task.id.clone();
                        let selected = self.columns[col_index].selected_id.clone();
                        let follow_created = may_follow && selected == selection_at_send;
                        if !self.board.upsert_task(&list_id, task) {
                            self.recover("create succeeded but its list disappeared".into());
                            return;
                        }
                        self.rebuild_column(&list_id);
                        if follow_created {
                            self.columns[col_index].select_id(Some(id));
                        }
                    }
                    Err(e) => {
                        if let Some(index) =
                            self.columns.iter().position(|column| column.list.id == *draft_list)
                        {
                            self.focus = index;
                        }
                        self.shelf = true;
                        self.field = FieldId::Title;
                        self.interaction = Interaction::Editing(draft);
                        self.recover(format!("create failed: {e}"));
                    }
                }
            }
            Msg::Deleted {
                operation,
                list_id,
                task_id,
                result,
            } => {
                let Some(context) = self.mutation_finished(operation) else {
                    return;
                };
                if !matches!(
                    &context,
                    MutationContext::Delete {
                        list_id: expected_list,
                        task_id: expected_task,
                        ..
                    } if expected_list == &list_id && expected_task == &task_id
                ) {
                    self.recover("delete response did not match the pending operation".into());
                    return;
                }
                match result {
                    Ok(()) => {
                    }
                    Err(e) => {
                        // The task is still there, so put the board back.
                        self.rollback(context);
                        self.recover(format!(
                            "delete failed for {list_id}/{task_id}: {e}"
                        ));
                    }
                }
            }
            Msg::Moved {
                operation,
                from,
                to,
                ids,
                result,
            } => {
                let Some(context) = self.mutation_finished(operation) else {
                    return;
                };
                if !matches!(
                    &context,
                    MutationContext::Move {
                        from: expected_from,
                        to: expected_to,
                        ids: expected_ids,
                        ..
                    } if expected_from == &from && expected_to == &to && expected_ids == &ids
                ) {
                    self.recover("move response did not match the pending operation".into());
                    return;
                }
                match (result.source, result.destination) {
                    (Ok(source), Ok(destination)) => {
                        self.board.replace_tasks(&from, source);
                        self.board.replace_tasks(&to, destination);
                        self.rebuild_column(&from);
                        self.rebuild_column(&to);
                        if let Some(failure) = result.failure {
                            self.recover(format!(
                                "move partially completed ({} moved); {} failed: {}",
                                result.moved.len(),
                                failure.task_id,
                                failure.error
                            ));
                        }
                    }
                    (source, destination) => {
                        self.rollback(context);
                        self.recover(format!(
                            "move reconciliation failed: source {}; destination {}",
                            source.err().unwrap_or_else(|| "ok".into()),
                            destination.err().unwrap_or_else(|| "ok".into())
                        ));
                    }
                }
            }
            Msg::Updated {
                operation,
                list_id,
                task_id,
                result,
            } => {
                let Some(context) = self.mutation_finished(operation) else {
                    return;
                };
                if !matches!(
                    &context,
                    MutationContext::Patch {
                        list_id: expected_list,
                        task_id: expected_task,
                        ..
                    } if expected_list == &list_id && expected_task == &task_id
                ) {
                    self.recover("save response did not match the pending operation".into());
                    return;
                }
                match result {
                    Ok(task) => {
                        let Some(col_index) =
                            self.columns.iter().position(|c| c.list.id == list_id)
                        else {
                            self.recover("save succeeded but its list disappeared".into());
                            return;
                        };
                        if task.id != task_id
                            || !self.columns[col_index]
                            .rows
                            .iter()
                            .any(|row| row.task.id == task_id)
                        {
                            self.recover("save succeeded but its task disappeared".into());
                            return;
                        }
                        self.board.upsert_task(&list_id, task);
                        self.rebuild_column(&list_id);
                    }
                    Err(e) => {
                        // The optimistic title may be wrong now, so resync.
                        self.rollback(context);
                        self.recover(format!("save failed: {e}"));
                    }
                }
            }
        }
    }

    fn move_task(&mut self, delta: isize) {
        let Some(col) = self.columns.get_mut(self.focus) else { return };
        let Some(last) = col.rows.len().checked_sub(1) else { return };
        let cur = col.state.selected().unwrap_or(0) as isize;
        col.select(Some((cur + delta).clamp(0, last as isize) as usize));
    }

    fn selected_task(&self) -> Option<&Row> {
        self.columns.get(self.focus)?.selected()
    }

    /// Copy the selected task as markdown, ready to paste into a note.
    fn yank(&mut self) {
        let Some(row) = self.selected_task() else { return };
        // The same control characters that would wreck a row would paste badly.
        let md = md_yank(&row.task);
        let title = title_of(&row.task).into_owned();
        match platform::copy(&md) {
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

    fn start_browse(&mut self) {
        let Some(row) = self.selected_task() else { return };
        let links = task_links(&row.task);
        if links.is_empty() {
            self.error = Some("no links on this task".into());
            return;
        }
        self.error = None;
        self.interaction = Interaction::Browse(Browse {
            links,
            state: ListState::default().with_selected(Some(0)),
        });
    }

    fn move_browse(&mut self, delta: isize) {
        let Interaction::Browse(b) = &mut self.interaction else { return };
        let Some(last) = b.links.len().checked_sub(1) else { return };
        let cur = b.state.selected().unwrap_or(0) as isize;
        b.state.select(Some((cur + delta).clamp(0, last as isize) as usize));
    }

    fn open_browse(&mut self, all: bool) {
        let urls = {
            let Interaction::Browse(b) = &self.interaction else { return };
            if all {
                b.links.iter().map(|l| l.url.clone()).collect()
            } else {
                let Some(i) = b.state.selected() else { return };
                let Some(l) = b.links.get(i) else { return };
                vec![l.url.clone()]
            }
        };
        for url in &urls {
            if let Err(error) = platform::open_url(url) {
                self.error = Some(format!("could not open {url}: {error}"));
                return;
            }
        }
        self.interaction = Interaction::Idle;
    }

    fn start_move(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(col) = self.columns.get(self.focus) else { return };
        let Some(selected) = col.selected() else {
            return;
        };
        if selected.task.assignment_info.is_some()
            || col.rows.iter().any(|row| {
                row.task.parent.as_deref() == Some(selected.task.id.as_str())
                    && row.task.assignment_info.is_some()
            })
        {
            self.error = Some("assigned tasks cannot move between lists".into());
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
        self.interaction = Interaction::Move(MovePick {
            lists,
            state: ListState::default().with_selected(Some(0)),
        });
    }

    fn step_move(&mut self, delta: isize) {
        let Interaction::Move(m) = &mut self.interaction else { return };
        let Some(last) = m.lists.len().checked_sub(1) else { return };
        let cur = m.state.selected().unwrap_or(0) as isize;
        m.state.select(Some((cur + delta).clamp(0, last as isize) as usize));
    }

    fn confirm_move(&mut self) {
        let dest_id = {
            let Interaction::Move(m) = &self.interaction else { return };
            let Some(i) = m.state.selected() else { return };
            let Some(list) = m.lists.get(i) else { return };
            list.id.clone()
        };
        self.interaction = Interaction::Idle;
        let Some(src) = self.columns.get(self.focus) else { return };
        let from_id = src.list.id.clone();
        let Some(row) = src.selected() else { return };
        let task_id = row.task.id.clone();
        let ids = move_ids(&src.rows, &task_id);
        if ids.is_empty() {
            return;
        }
        let Some(dest_idx) = self.columns.iter().position(|c| c.list.id == dest_id) else {
            return;
        };
        let source_before = self.board.tasks(&from_id);
        let destination_before = self.board.tasks(&dest_id);
        self.board.move_tree(&from_id, &dest_id, &task_id);
        self.rebuild_column(&from_id);
        self.rebuild_column(&dest_id);
        self.columns[dest_idx].select_id(Some(task_id.clone()));
        self.focus = dest_idx;
        self.error = None;
        self.send_mutation(
            MutationContext::Move {
                from: from_id.clone(),
                to: dest_id.clone(),
                ids: ids.clone(),
                source_before,
                destination_before,
            },
            |operation| Cmd::Move {
                operation,
                from: from_id,
                to: dest_id,
                ids,
            },
        );
    }

    fn start_help(&mut self) {
        self.interaction = Interaction::Help;
    }

    fn on_key(&mut self, k: KeyEvent) {
        // A notice is for the keypress that earned it, and no longer.
        self.notice = None;
        if k.code == KeyCode::Char('c') && k.modifiers == KeyModifiers::CONTROL {
            self.request_quit();
            return;
        }
        if self.quitting {
            if no_modifiers(k.modifiers)
                && matches!(k.code, KeyCode::Esc | KeyCode::Char('q'))
            {
                self.request_quit();
            }
            return;
        }
        if matches!(self.interaction, Interaction::Editing(_)) {
            self.edit_key(k);
            return;
        }
        if matches!(self.interaction, Interaction::ConfirmDelete(_)) {
            // Only "y" deletes; every other key cancels.
            match k.code {
                KeyCode::Char('y') if no_modifiers(k.modifiers) => self.confirm_delete(),
                _ => self.interaction = Interaction::Idle,
            }
            return;
        }
        if matches!(self.interaction, Interaction::Browse(_)) {
            match k.code {
                KeyCode::Esc | KeyCode::Char('q') if no_modifiers(k.modifiers) => {
                    self.interaction = Interaction::Idle
                }
                KeyCode::Char('j') | KeyCode::Down if no_modifiers(k.modifiers) => {
                    self.move_browse(1)
                }
                KeyCode::Char('k') | KeyCode::Up if no_modifiers(k.modifiers) => {
                    self.move_browse(-1)
                }
                KeyCode::Enter if k.modifiers == KeyModifiers::SHIFT => {
                    self.open_browse(true)
                }
                KeyCode::Enter if no_modifiers(k.modifiers) => self.open_browse(false),
                _ => {}
            }
            return;
        }
        if matches!(self.interaction, Interaction::Move(_)) {
            match k.code {
                KeyCode::Esc | KeyCode::Char('q') if no_modifiers(k.modifiers) => {
                    self.interaction = Interaction::Idle
                }
                KeyCode::Char('j') | KeyCode::Down if no_modifiers(k.modifiers) => {
                    self.step_move(1)
                }
                KeyCode::Char('k') | KeyCode::Up if no_modifiers(k.modifiers) => {
                    self.step_move(-1)
                }
                KeyCode::Enter if no_modifiers(k.modifiers) => self.confirm_move(),
                _ => {}
            }
            return;
        }
        if matches!(self.interaction, Interaction::Help) {
            match k.code {
                KeyCode::Esc | KeyCode::Char('q') if no_modifiers(k.modifiers) => {
                    self.interaction = Interaction::Idle
                }
                KeyCode::Char('?') if shifted(k.modifiers) => {
                    self.interaction = Interaction::Idle
                }
                _ => {}
            }
            return;
        }
        match k.code {
            KeyCode::Char('q') if no_modifiers(k.modifiers) => self.request_quit(),
            KeyCode::Esc if no_modifiers(k.modifiers) && self.shelf => self.shelf = false,
            KeyCode::Esc if no_modifiers(k.modifiers) => self.request_quit(),
            KeyCode::Char('h') | KeyCode::Left if no_modifiers(k.modifiers) => {
                self.focus = self.focus.saturating_sub(1)
            }
            KeyCode::Char('l') | KeyCode::Right if no_modifiers(k.modifiers) => {
                self.focus = (self.focus + 1).min(self.columns.len().saturating_sub(1))
            }
            KeyCode::Char('j') | KeyCode::Down if no_modifiers(k.modifiers) => self.move_task(1),
            KeyCode::Char('k') | KeyCode::Up if no_modifiers(k.modifiers) => self.move_task(-1),
            KeyCode::Char('g') | KeyCode::Home if no_modifiers(k.modifiers) => {
                self.move_task(isize::MIN / 2)
            }
            KeyCode::Char('G') if shifted(k.modifiers) => self.move_task(isize::MAX / 2),
            KeyCode::End if no_modifiers(k.modifiers) => self.move_task(isize::MAX / 2),
            KeyCode::Enter if no_modifiers(k.modifiers) => {
                self.shelf = !self.shelf && self.selected_task().is_some();
                self.field = FieldId::Title;
            }
            KeyCode::Tab if no_modifiers(k.modifiers) && self.shelf => {
                self.field = match self.field {
                    FieldId::Title => FieldId::Notes,
                    FieldId::Notes => FieldId::Title,
                }
            }
            KeyCode::BackTab if shifted(k.modifiers) && self.shelf => {
                self.field = match self.field {
                    FieldId::Title => FieldId::Notes,
                    FieldId::Notes => FieldId::Title,
                }
            }
            KeyCode::Char('i') if no_modifiers(k.modifiers) => self.start_edit(),
            KeyCode::Char(' ') if no_modifiers(k.modifiers) => self.toggle_done(),
            KeyCode::Char('d') if no_modifiers(k.modifiers) => self.ask_delete(),
            KeyCode::Char('c') if no_modifiers(k.modifiers) => self.start_create(false),
            KeyCode::Char('C') if shifted(k.modifiers) => self.start_create(true),
            KeyCode::Char('r') if no_modifiers(k.modifiers) => self.refresh(),
            KeyCode::Char('t') if no_modifiers(k.modifiers) => {
                self.transparent = !self.transparent
            }
            KeyCode::Char('y') if no_modifiers(k.modifiers) => self.yank(),
            KeyCode::Char('b') if no_modifiers(k.modifiers) => self.start_browse(),
            KeyCode::Char('m') if no_modifiers(k.modifiers) => self.start_move(),
            KeyCode::Char('?') if shifted(k.modifiers) => self.start_help(),
            _ => {}
        }
    }

    /// Keys while editing a field. Enter saves, Esc discards; in notes,
    /// Shift+Enter (or Alt+Enter / Ctrl+J where the terminal can't report it)
    /// starts a new line.
    fn edit_key(&mut self, k: KeyEvent) {
        if self.edit_width == 0
            && !(k.code == KeyCode::Esc && no_modifiers(k.modifiers))
        {
            self.error = Some("terminal too small to show the editor; Esc cancels".into());
            return;
        }
        let ctrl = k.modifiers == KeyModifiers::CONTROL;
        let newline =
            k.modifiers == KeyModifiers::SHIFT || k.modifiers == KeyModifiers::ALT;
        let notes = self.edit().is_some_and(|e| e.field() == FieldId::Notes);
        match k.code {
            KeyCode::Esc if no_modifiers(k.modifiers) => {
                self.interaction = Interaction::Idle;
                return;
            }
            KeyCode::Enter if no_modifiers(k.modifiers) => {
                self.commit_edit();
                return;
            }
            _ => {}
        }
        let width = self.edit_width.max(1);
        let Some(e) = self.edit_mut() else { return };
        match k.code {
            KeyCode::Enter if notes && newline => e.insert('\n'),
            KeyCode::Char('j') if ctrl && notes => e.insert('\n'),
            KeyCode::Char('w') if ctrl => e.kill_word(),
            KeyCode::Char('u') if ctrl => e.kill_to_start(),
            KeyCode::Char('a') if ctrl => e.cursor = e.line_start(),
            KeyCode::Char('e') if ctrl => e.cursor = e.line_end(),
            KeyCode::Char(c)
                if no_modifiers(k.modifiers)
                    || k.modifiers == KeyModifiers::SHIFT =>
            {
                e.insert(c)
            }
            KeyCode::Backspace if no_modifiers(k.modifiers) => e.backspace(),
            KeyCode::Delete if no_modifiers(k.modifiers) => e.delete(),
            KeyCode::Left if no_modifiers(k.modifiers) => e.left(),
            KeyCode::Right if no_modifiers(k.modifiers) => e.right(),
            KeyCode::Up if notes && no_modifiers(k.modifiers) => {
                e.step_visual_line(width, false)
            }
            KeyCode::Down if notes && no_modifiers(k.modifiers) => {
                e.step_visual_line(width, true)
            }
            KeyCode::Home if no_modifiers(k.modifiers) => e.cursor = e.line_start(),
            KeyCode::End if no_modifiers(k.modifiers) => e.cursor = e.line_end(),
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame) {
        self.edit_width = 0;
        let t = self.theme.clone();
        let base = Style::default().fg(t.fg).bg(self.bg(t.bg));
        f.render_widget(Block::default().style(base), f.area());

        let [header, body, status] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(f.area());

        let mut head = vec![
            Span::styled(" Tasks ", Style::default().fg(t.bg).bg(t.accent).bold()),
            Span::styled(format!("  theme: {}", t.name), Style::default().fg(t.muted)),
        ];
        if self.pending_operation.is_some() {
            head.push(Span::styled("  saving…", Style::default().fg(t.yellow)));
        } else if self.loading() {
            head.push(Span::styled("  syncing…", Style::default().fg(t.yellow)));
        }
        f.render_widget(Line::from(head), header);

        self.draw_board(f, body, &t);
        self.draw_browse(f, body, &t);
        self.draw_move(f, body, &t);
        self.draw_help(f, body, &t);

        let hints = if self.quitting {
            " waiting for pending saves… q again to force quit"
        } else if matches!(self.interaction, Interaction::Browse(_)) {
            " j/k · ⏎ open · ⇧⏎ all · Esc close"
        } else if matches!(self.interaction, Interaction::Move(_)) {
            " j/k · ⏎ move · Esc close"
        } else if matches!(self.interaction, Interaction::Help) {
            " Esc close"
        } else if self.is_creating() {
            " ⏎ create · Esc cancel"
        } else if self.edit().is_some_and(|e| e.field() == FieldId::Notes) {
            " ⏎ save · ⇧⏎ new line · Esc cancel · ^W delete word"
        } else if self.edit().is_some() {
            " ⏎ save · Esc cancel · ^W delete word · ^U clear"
        } else if self.sync != SyncState::Ready {
            " syncing with Google… board is read-only until it lands"
        } else if self.shelf {
            " Tab field · i edit · c new · C subtask · ␣ done · y copy · b link · m move · d delete · ? help · q quit"
        } else {
            " h/l list · j/k task · ␣ done · ⏎ details · c new · C subtask · i edit · y copy · b link · m move · d delete · ? help · q quit"
        };
        let pending = match &self.interaction {
            Interaction::ConfirmDelete(pending) => Some(pending),
            _ => None,
        };
        let status_line = match (pending, &self.error, &self.notice) {
            (Some(p), _, _) => {
                Line::styled(format!(" {}", p.prompt), Style::default().fg(t.yellow).bold())
            }
            (None, Some(e), _) => Line::styled(format!(" {e}"), Style::default().fg(t.red)),
            (None, None, Some(n)) => Line::styled(format!(" {n}"), Style::default().fg(t.green)),
            (None, None, None) => Line::styled(hints, Style::default().fg(t.muted)),
        };
        f.render_widget(status_line, status);
    }

    fn draw_board(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        if self.columns.is_empty() {
            let msg = if self.loading() { " Loading task lists…" } else { " No task lists" };
            f.render_widget(Paragraph::new(msg).style(Style::default().fg(t.muted)), area);
            return;
        }
        self.focus = self.focus.min(self.columns.len() - 1);
        if self.selected_task().is_none() && !self.is_creating() {
            self.shelf = false;
            if matches!(
                self.interaction,
                Interaction::Editing(Edit {
                    target: EditTarget::Existing { .. },
                    ..
                })
            ) {
                self.interaction = Interaction::Idle;
            }
        }

        if self.shelf && area.width < COL_MIN + 24 {
            self.draw_shelf(f, area, t);
            return;
        }

        // Columns share the width evenly within limits; the shelf takes a fixed slot.
        let shelf_w = if self.shelf { SHELF_W.min(area.width / 2) } else { 0 };
        let col_w = ((area.width - shelf_w) / self.columns.len() as u16).clamp(COL_MIN, COL_MAX);

        // Scroll just enough to keep the focused column (and its shelf) in view.
        self.scroll = self.scroll.min(self.focus);
        let span = |from: usize| (self.focus - from + 1) as u16 * col_w + shelf_w;
        while self.scroll < self.focus && span(self.scroll) > area.width {
            self.scroll += 1;
        }

        let right = area.right();
        let mut x = area.x;
        for i in self.scroll..self.columns.len() {
            if x >= right {
                break;
            }
            let w = col_w.min(right - x);
            self.draw_column(f, Rect::new(x, area.y, w, area.height), i, t);
            x += w;
            if self.shelf && i == self.focus && x < right {
                let w = shelf_w.min(right - x);
                self.draw_shelf(f, Rect::new(x, area.y, w, area.height), t);
                x += w;
            }
        }
    }

    fn draw_column(&mut self, f: &mut Frame, area: Rect, i: usize, t: &Theme) {
        let focused = i == self.focus;
        let surface = self.bg(t.surface);
        let today = Local::now().date_naive();
        // Two borders and the selection marker sit outside the item's text.
        let wrap_to = self
            .config
            .wrap_titles
            .then(|| (area.width as usize).saturating_sub(3));
        let col = &mut self.columns[i];

        let open = col.rows.iter().filter(|r| !r.task.is_done()).count();
        let title_fg = if focused { t.fg } else { t.muted };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(if focused { t.accent } else { t.muted }))
            .title(Line::styled(
                format!(" {} ", clean(&col.list.title)),
                Style::default().fg(title_fg).bold(),
            ))
            .title(Line::styled(format!(" {open} "), Style::default().fg(t.muted)).right_aligned())
            .style(Style::default().bg(surface));

        if col.rows.is_empty() {
            let msg = if col.load == LoadState::Loading {
                "Loading…"
            } else {
                "No tasks"
            };
            f.render_widget(Paragraph::new(msg).style(Style::default().fg(t.muted)).block(block), area);
            return;
        }

        let items: Vec<ListItem> =
            col.rows.iter().map(|r| task_item(r, t, today, wrap_to)).collect();
        let highlight = if focused {
            Style::default().fg(t.sel_fg).bg(t.sel_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let list = List::new(items)
            .block(block)
            .highlight_style(highlight)
            .highlight_symbol(if focused { "▌" } else { " " })
            .highlight_spacing(HighlightSpacing::Always);
        f.render_stateful_widget(list, area, &mut col.state);
    }

    /// The shelf while a new task is being typed: just a title input.
    fn draw_new_shelf(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let col = &self.columns[self.focus];
        let parent_id = match self.edit().map(|edit| &edit.target) {
            Some(EditTarget::New { parent, .. }) => parent.clone(),
            _ => return,
        };
        let parent_ref = parent_id.as_deref();
        let parent = parent_ref.and_then(|id| col.rows.iter().find(|r| r.task.id == id));
        let heading = if parent_ref.is_some() { " New subtask " } else { " New task " };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(t.green))
            .title(Line::styled(heading, Style::default().fg(t.green).bold()))
            .padding(Padding::horizontal(1))
            .style(Style::default().bg(self.bg(t.bg)));
        let inner = block.inner(area);
        f.render_widget(block, area);
        if inner.width < 8 || inner.height < 3 {
            f.render_widget(
                Paragraph::new("Terminal too small; Esc cancels")
                    .style(Style::default().fg(t.yellow)),
                inner,
            );
            return;
        }
        let w = inner.width as usize - 2;
        self.edit_width = w;
        let Some(e) = self.edit() else { return };
        let (text, offset) = e.view(w);
        let text = clean(text);
        let input = Line::styled(
            pad_to_width(&text, w),
            Style::default().fg(t.sel_fg).bg(t.sel_bg).bold(),
        );
        let caption = match (parent, parent_ref.is_some()) {
            (Some(p), _) => format!("  under {}", title_of(&p.task)),
            (None, true) => "  under the selected task".to_string(),
            (None, false) => format!("  in {}", col.list.title),
        };
        let lines = vec![
            marked(vec![input], true, t).remove(0),
            Line::default(),
            Line::styled(caption, Style::default().fg(t.muted)),
        ];
        f.render_widget(Paragraph::new(lines), inner);
        f.set_cursor_position((inner.x + 2 + offset as u16, inner.y));
    }

    fn draw_shelf(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        if self.is_creating() {
            self.draw_new_shelf(f, area, t);
            return;
        }
        let col = &self.columns[self.focus];
        let Some(row) = col.selected() else { return };
        let task = &row.task;
        let today = Local::now().date_naive();
        let muted = Style::default().fg(t.muted);

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(t.accent))
            .title(Line::styled(" Details ", Style::default().fg(t.accent).bold()))
            .padding(Padding::horizontal(1))
            .style(Style::default().bg(self.bg(t.bg)));
        let inner = block.inner(area);
        f.render_widget(block, area);
        if inner.width < 8 || inner.height < 8 {
            f.render_widget(
                Paragraph::new("Terminal too small; Esc closes details")
                    .style(Style::default().fg(t.yellow)),
                inner,
            );
            return;
        }
        // Two columns go to the focus marker.
        let w = inner.width as usize - 2;
        if self.edit().is_some() {
            self.edit_width = w;
        }
        let editing = self.edit().filter(|e| {
            e.existing()
                .is_some_and(|(list_id, task_id, _)| task_id == task.id && list_id == col.list.id)
        });
        let editing_field = editing.map(Edit::field);
        let input = Style::default().fg(t.sel_fg).bg(t.sel_bg);
        let mut cursor = None;

        let mut meta = Vec::new();
        let (status, color) = if task.is_done() { ("Completed", t.green) } else { ("Open", t.blue) };
        meta.push(field("Status", Span::styled(status, Style::default().fg(color)), t));
        if let Some(d) = task.due_date() {
            let label = format!("{} · {}", d.format("%a %b %-d, %Y"), relative(d, today));
            let color = if task.is_done() { t.fg } else { due_color(d, today, t) };
            meta.push(field("Due", Span::styled(label, Style::default().fg(color)), t));
        }
        if let Some(c) = &task.completed {
            meta.push(field("Completed", fmt_time(c).into(), t));
        }
        if let Some(p) = task.parent.as_deref().and_then(|p| col.rows.iter().find(|r| r.task.id == p)) {
            meta.push(field("Parent", title_of(&p.task).to_string().into(), t));
        }
        let subtasks: Vec<&Task> =
            col.rows.iter().map(|r| &r.task).filter(|c| c.parent.as_deref() == Some(task.id.as_str())).collect();
        if !subtasks.is_empty() {
            let done = subtasks.iter().filter(|s| s.is_done()).count();
            meta.push(field("Subtasks", format!("{done}/{} done", subtasks.len()).into(), t));
        }
        if task.hidden {
            meta.push(field("Hidden", "yes".into(), t));
        }
        for l in &task.links {
            let text = if l.description.is_empty() { &l.link } else { &l.description };
            meta.push(field(
                "Link",
                Span::styled(clean(text).into_owned(), Style::default().fg(t.blue)),
                t,
            ));
        }

        // title · gap · meta · gap · "Notes" · notes · gap · updated
        let title_h = match editing_field {
            Some(FieldId::Title) => 1,
            _ => wrap(&title_of(task), w).len().min(3) as u16,
        };
        let max_meta = inner.height.saturating_sub(title_h + 7).max(1) as usize;
        if meta.len() > max_meta {
            meta.truncate(max_meta);
            if let Some(last) = meta.last_mut() {
                *last = field("More", "…".into(), t);
            }
        }
        let meta_h = meta.len() as u16;
        let notes_h = inner.height.saturating_sub(title_h + meta_h + 5).max(1);
        let [title_a, _, meta_a, _, notes_label_a, notes_a, _, updated_a] = Layout::vertical([
            Constraint::Length(title_h),
            Constraint::Length(1),
            Constraint::Length(meta_h),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(notes_h),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(inner);

        let title = title_of(task);
        let title_lines = match editing.filter(|e| e.field() == FieldId::Title) {
            Some(e) => {
                let (text, offset) = e.view(w);
                cursor = Some((title_a.x + 2 + offset as u16, title_a.y));
                vec![Line::styled(
                    pad_to_width(&clean(text), w),
                    input.bold(),
                )]
            }
            None => wrap(&title, w)
                .into_iter()
                .take(title_h as usize)
                .map(|(s, e)| Line::styled(title[s..e].to_string(), Style::default().fg(t.fg).bold()))
                .collect(),
        };
        let on_title = self.field == FieldId::Title;
        f.render_widget(Paragraph::new(marked(title_lines, on_title, t)), title_a);
        f.render_widget(Paragraph::new(marked(meta, false, t)), meta_a);
        f.render_widget(Line::styled("  Notes", muted.bold()), notes_label_a);

        let stored = task.notes.as_deref().unwrap_or("");
        let raw = editing
            .filter(|e| e.field() == FieldId::Notes)
            .map_or(stored, |e| &e.buf);
        let cleaned = clean(raw);
        let notes_text = cleaned.as_ref();
        let rows = notes_a.height as usize;
        let notes_lines = if notes_text.is_empty() && editing_field != Some(FieldId::Notes) {
            vec![Line::styled("(none)", muted)]
        } else {
            let segs = wrap(notes_text, w);
            let mut scroll = 0;
            if let Some(e) = editing.filter(|e| e.field() == FieldId::Notes) {
                let (row, col) = cursor_rc(&segs, notes_text, e.cursor);
                scroll = (row + 1).saturating_sub(rows);
                cursor = Some((notes_a.x + 2 + col as u16, notes_a.y + (row - scroll) as u16));
            }
            let style = match editing_field {
                Some(FieldId::Notes) => input,
                _ => Style::default().fg(t.fg),
            };
            segs.into_iter()
                .skip(scroll)
                .take(rows)
                .map(|(s, e)| Line::styled(pad_to_width(&notes_text[s..e], w), style))
                .collect()
        };
        let on_notes = self.field == FieldId::Notes;
        f.render_widget(Paragraph::new(marked(notes_lines, on_notes, t)), notes_a);
        f.render_widget(
            Paragraph::new(marked(vec![field("Updated", fmt_time(&task.updated).into(), t)], false, t)),
            updated_a,
        );
        if let Some(pos) = cursor {
            f.set_cursor_position(pos);
        }
    }

    fn draw_browse(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let bg = self.bg(t.bg);
        let wrap_titles = self.config.wrap_titles;
        let Interaction::Browse(browse) = &mut self.interaction else { return };
        let rect = modal_rect(area, 80, 3);
        // Two borders and the selection marker eat into the usable width.
        let text_w = (rect.width as usize).saturating_sub(4).max(1);
        let items: Vec<ListItem> = browse
            .links
            .iter()
            .map(|l| {
                // A bare URL from the notes has no title to put above it.
                let Some(name) = &l.name else {
                    return ListItem::new(Line::styled(
                        ellipsize(&l.url, text_w),
                        Style::default().fg(t.blue),
                    ));
                };
                let style = Style::default().fg(t.fg).bold();
                let mut lines: Vec<Line> = if wrap_titles {
                    wrap(name, text_w)
                        .into_iter()
                        .map(|(from, to)| Line::styled(name[from..to].to_string(), style))
                        .collect()
                } else {
                    vec![Line::styled(ellipsize(name, text_w), style)]
                };
                // The URL stays on one line: wrapping one only makes it harder
                // to read, and it is a subtitle here, not the label.
                lines.push(Line::styled(
                    format!("  {}", ellipsize(&l.url, text_w.saturating_sub(2))),
                    Style::default().fg(t.muted),
                ));
                ListItem::new(lines)
            })
            .collect();
        let rows: u16 = items.iter().map(|i| i.height() as u16).sum();
        let rect = modal_rect(area, 80, rows.saturating_add(2));
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(t.accent))
            .title(Line::styled(" Browse Link ", Style::default().fg(t.accent).bold()))
            .style(Style::default().bg(bg));
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::default().fg(t.sel_fg).bg(t.sel_bg).add_modifier(Modifier::BOLD))
            .highlight_symbol("▌")
            .highlight_spacing(HighlightSpacing::Always);
        f.render_widget(Clear, rect); // a Block restyles cells but leaves their text
        f.render_stateful_widget(list, rect, &mut browse.state);
    }

    fn draw_move(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let bg = self.bg(t.bg);
        let Interaction::Move(pick) = &mut self.interaction else { return };
        let rect = modal_rect(area, 72, pick.lists.len() as u16 + 2);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(t.accent))
            .title(Line::styled(" Move ", Style::default().fg(t.accent).bold()))
            .style(Style::default().bg(bg));
        let items: Vec<ListItem> = pick
            .lists
            .iter()
            .map(|l| {
                ListItem::new(Line::styled(
                    clean(&l.title).into_owned(),
                    Style::default().fg(t.fg),
                ))
            })
            .collect();
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::default().fg(t.sel_fg).bg(t.sel_bg).add_modifier(Modifier::BOLD))
            .highlight_symbol("▌")
            .highlight_spacing(HighlightSpacing::Always);
        f.render_widget(Clear, rect);
        f.render_stateful_widget(list, rect, &mut pick.state);
    }

    fn draw_help(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let bg = self.bg(t.bg);
        if !matches!(self.interaction, Interaction::Help) {
            return;
        }
        let keys = help_keys();
        let key_w = keys
            .iter()
            .map(|(key, _)| UnicodeWidthStr::width(*key))
            .max()
            .unwrap_or(0);
        let rect = modal_rect(area, 56, keys.len() as u16 + 2);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(t.accent))
            .title(Line::styled(" Keyboard ", Style::default().fg(t.accent).bold()))
            .style(Style::default().bg(bg));
        let items: Vec<ListItem> = keys
            .iter()
            .map(|(k, d)| {
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {k:<key_w$}  "), Style::default().fg(t.accent).bold()),
                    Span::styled(*d, Style::default().fg(t.fg)),
                ]))
            })
            .collect();
        f.render_widget(Clear, rect);
        f.render_widget(List::new(items).block(block), rect);
    }
}

fn modal_rect(area: Rect, max_width: u16, desired_height: u16) -> Rect {
    let width = area.width.min(max_width);
    let height = area.height.min(desired_height.max(3));
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn no_modifiers(modifiers: KeyModifiers) -> bool {
    modifiers.is_empty()
}

fn shifted(modifiers: KeyModifiers) -> bool {
    modifiers == KeyModifiers::SHIFT
}

fn help_keys() -> Vec<(&'static str, &'static str)> {
    vec![
        ("h / l", "previous / next list"),
        ("j / k", "previous / next task"),
        ("g / G", "first / last task"),
        ("Enter", "open / close details"),
        ("Tab", "title / notes (shelf)"),
        ("Space", "tick / untick"),
        ("c", "new task"),
        ("C", "new subtask"),
        ("i", "edit focused field"),
        ("d", "delete task"),
        ("y", "copy task as markdown"),
        ("b", "browse links in notes"),
        ("m", "move to another list"),
        ("r", "refresh from Google"),
        ("t", "transparent background"),
        ("?", "keyboard commands"),
        ("q / Esc", "quit (Esc closes overlays)"),
        ("⇧Enter", "new line (notes) / open all (browse)"),
    ]
}

fn other_lists<'a>(lists: &'a [TaskList], current_id: &str) -> Vec<&'a TaskList> {
    lists.iter().filter(|l| l.id != current_id).collect()
}

/// The selected task, then its subtasks. Those go with it to the dest list.
fn move_ids(rows: &[Row], task_id: &str) -> Vec<String> {
    if !rows.iter().any(|r| r.task.id == task_id) {
        return Vec::new();
    }
    let mut ids = vec![task_id.to_string()];
    ids.extend(
        rows.iter()
            .filter(|r| r.task.parent.as_deref() == Some(task_id))
            .map(|r| r.task.id.clone()),
    );
    ids
}

/// Where a new subtask hangs: off the row itself when it is top-level, or
/// off the row's own parent when it is already a subtask — Google Tasks nests
/// exactly one level deep, so a subtask's sibling is the closest thing to a
/// child of it.
fn subtask_parent(row: &Row) -> Option<String> {
    match row.depth {
        0 => Some(row.task.id.clone()),
        _ => row.task.parent.clone(),
    }
}

/// Everything the task links to: the links Google attached to it (a Gmail
/// thread, a Doc) keep the source's title, then any bare URLs typed into the
/// notes. First-seen order, no duplicate URLs.
fn task_links(task: &Task) -> Vec<BrowseLink> {
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

/// Cut `s` to terminal cells, marking the cut with an ellipsis.
fn ellipsize(s: &str, width: usize) -> String {
    if UnicodeWidthStr::width(s) <= width {
        return s.to_string();
    }
    let target = width.saturating_sub(1);
    let mut out = String::new();
    for grapheme in s.graphemes(true) {
        if UnicodeWidthStr::width(out.as_str()) + UnicodeWidthStr::width(grapheme) > target {
            break;
        }
        out.push_str(grapheme);
    }
    out.push('…');
    out
}

fn pad_to_width(text: &str, width: usize) -> String {
    let padding = width.saturating_sub(UnicodeWidthStr::width(text));
    format!("{text}{}", " ".repeat(padding))
}

/// Greedy word wrap. Returns the byte range of each display line, so the
/// caller can map a cursor offset onto the same layout it renders.
fn wrap(text: &str, width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut base = 0;
    for line in text.split('\n') {
        let mut start = 0;
        loop {
            let rest = &line[start..];
            if UnicodeWidthStr::width(rest) <= width {
                out.push((base + start, base + line.len()));
                break;
            }
            let mut cells = 0;
            let mut hard = 0;
            let mut whitespace = None;
            for (offset, grapheme) in rest.grapheme_indices(true) {
                let next = cells + UnicodeWidthStr::width(grapheme);
                if next > width && hard > 0 {
                    break;
                }
                cells = next;
                hard = offset + grapheme.len();
                if grapheme.chars().all(char::is_whitespace) {
                    whitespace = Some(hard);
                }
                if cells >= width {
                    break;
                }
            }
            let cut = whitespace.unwrap_or(hard).max(1);
            out.push((base + start, base + start + cut));
            start += cut;
        }
        base += line.len() + 1; // skip the '\n'
    }
    out
}

/// Where `cursor` lands in the wrapped layout, as (line, column). The last
/// row starting at or before the cursor wins, so a cursor sitting at the end
/// of a line (no segment covers the '\n') stays on that line.
fn cursor_rc(segs: &[(usize, usize)], text: &str, cursor: usize) -> (usize, usize) {
    let mut out = (0, 0);
    for (row, (s, e)) in segs.iter().enumerate() {
        if cursor < *s {
            break;
        }
        out = (
            row,
            UnicodeWidthStr::width(&text[*s..cursor.min(*e)]),
        );
    }
    out
}

fn byte_at_column(text: &str, start: usize, end: usize, column: usize) -> usize {
    let mut cells = 0;
    for (offset, grapheme) in text[start..end].grapheme_indices(true) {
        let next = cells + UnicodeWidthStr::width(grapheme);
        if next > column {
            return start + offset;
        }
        cells = next;
    }
    end
}

/// Prefix each line with the focus marker so focused fields stand out.
fn marked<'a>(lines: Vec<Line<'a>>, focused: bool, t: &Theme) -> Vec<Line<'a>> {
    lines
        .into_iter()
        .map(|l| {
            let mark = Span::styled(if focused { "▌ " } else { "  " }, Style::default().fg(t.accent));
            Line::from([vec![mark], l.spans].concat())
        })
        .collect()
}

fn title_of(task: &Task) -> Cow<'_, str> {
    if task.title.trim().is_empty() {
        Cow::Borrowed("(untitled)")
    } else {
        clean(&task.title)
    }
}

/// Task text arrives from Google as-is, control characters and all. Printing
/// one is not cosmetic: a stray `\r` in a title walks the terminal cursor back
/// to column 0 and every cell after it lands in the wrong place. Clipping used
/// to hide them past the edge of a column; wrapping renders the whole string,
/// so they have to go. Every ASCII control character is one byte, so swapping
/// in a space keeps the byte offsets `wrap()` and `cursor_rc()` hand around.
fn clean(s: &str) -> Cow<'_, str> {
    if !s.bytes().any(|b| b.is_ascii_control() && b != b'\n') {
        return Cow::Borrowed(s);
    }
    Cow::Owned(
        s.chars()
            .map(|c| if c.is_ascii_control() && c != '\n' { ' ' } else { c })
            .collect(),
    )
}

fn field<'a>(key: &str, value: Span<'a>, t: &Theme) -> Line<'a> {
    Line::from(vec![Span::styled(format!("{key:<11}"), Style::default().fg(t.muted)), value])
}

/// One row of a column. `wrap_to` is the usable text width when titles wrap,
/// `None` to keep each task on a single clipped line.
fn task_item<'a>(
    r: &'a Row,
    t: &Theme,
    today: NaiveDate,
    wrap_to: Option<usize>,
) -> ListItem<'a> {
    let task = &r.task;
    let indent = "  ".repeat(r.depth as usize);
    let done = task.is_done();
    let (mark, mark_fg, style) = if done {
        ("✓ ", t.green, Style::default().fg(t.muted).add_modifier(Modifier::CROSSED_OUT))
    } else {
        ("○ ", t.muted, Style::default().fg(t.fg))
    };
    let title = title_of(task);
    // The indent and the ○/✓ marker both eat into the room left for the title.
    let head = indent.chars().count() + 2;
    let segs = match wrap_to {
        Some(w) => wrap(&title, w.saturating_sub(head).max(1)),
        None => vec![(0, title.len())],
    };
    let noted = task.notes.as_deref().is_some_and(|n| !n.trim().is_empty());
    let last = segs.len() - 1;
    let mut lines: Vec<Line> = segs
        .iter()
        .enumerate()
        .map(|(i, &(from, to))| {
            // A break at a run of spaces leaves the next line starting on one;
            // that reads as a ragged indent, so drop it.
            let from = from + title[from..to].len() - title[from..to].trim_start().len();
            // Continuation lines sit under the title, past the marker.
            let mut spans = vec![Span::raw(match i {
                0 => indent.clone(),
                _ => format!("{indent}  "),
            })];
            if i == 0 {
                spans.push(Span::styled(mark, Style::default().fg(mark_fg)));
            }
            spans.push(Span::styled(title[from..to].to_string(), style));
            if i == last && noted {
                spans.push(Span::styled(" ≡", Style::default().fg(t.muted)));
            }
            Line::from(spans)
        })
        .collect();
    if let Some(d) = task.due_date().filter(|_| !done) {
        lines.push(Line::from(vec![
            Span::raw(format!("{indent}  ")),
            Span::styled(relative(d, today), Style::default().fg(due_color(d, today, t))),
        ]));
    }
    ListItem::new(lines)
}

fn relative(d: NaiveDate, today: NaiveDate) -> String {
    match (d - today).num_days() {
        0 => "Today".into(),
        1 => "Tomorrow".into(),
        -1 => "Yesterday".into(),
        n if n < 0 => format!("{} · {}d overdue", d.format("%b %-d"), -n),
        n if n < 7 => d.format("%A").to_string(),
        _ if d.year() == today.year() => d.format("%a %b %-d").to_string(),
        _ => d.format("%b %-d, %Y").to_string(),
    }
}

fn due_color(d: NaiveDate, today: NaiveDate, t: &Theme) -> Color {
    match (d - today).num_days() {
        n if n < 0 => t.red,
        0 => t.yellow,
        _ => t.muted,
    }
}

fn fmt_time(s: &str) -> String {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Local).format("%a %b %-d %Y, %-I:%M %p").to_string())
        .unwrap_or_else(|_| s.to_string())
}

/// Owns the API client so network calls never block the UI.
fn worker<A: TasksApi>(mut client: A, cmds: Receiver<Cmd>, out: Sender<Msg>) {
    for cmd in cmds {
        match cmd {
            Cmd::Create {
                operation,
                list_id,
                task,
                parent,
                selection,
            } => {
                let res = client
                    .create(&list_id, &task, parent.as_deref())
                    .map_err(|e| e.to_string());
                if out
                    .send(Msg::Created {
                        operation,
                        list_id,
                        selection,
                        result: res,
                    })
                    .is_err()
                {
                    return;
                }
            }
            Cmd::Delete {
                operation,
                list_id,
                task_id,
                etag,
            } => {
                let res = client
                    .delete(&list_id, &task_id, Some(&etag))
                    .map_err(|e| e.to_string());
                if out
                    .send(Msg::Deleted {
                        operation,
                        list_id,
                        task_id,
                        result: res,
                    })
                    .is_err()
                {
                    return;
                }
            }
            Cmd::Patch {
                operation,
                list_id,
                task_id,
                patch,
                etag,
            } => {
                let res = client
                    .patch(&list_id, &task_id, &patch, Some(&etag))
                    .map_err(|e| e.to_string());
                if out
                    .send(Msg::Updated {
                        operation,
                        list_id,
                        task_id,
                        result: res,
                    })
                    .is_err()
                {
                    return;
                }
            }
            Cmd::Move {
                operation,
                from,
                to,
                ids,
            } => {
                let mut moved = Vec::new();
                let mut failure = None;
                let requests = move_requests(&to, &ids);
                if requests.is_empty() {
                    failure = Some(MoveFailure {
                        task_id: String::new(),
                        error: "no task".into(),
                    });
                } else {
                    for (id, movement) in requests {
                        match client.move_task(&from, &id, &movement) {
                            Ok(task) => {
                                moved.push(task);
                            }
                            Err(error) => {
                                failure = Some(MoveFailure {
                                    task_id: id.clone(),
                                    error: error.to_string(),
                                });
                                break;
                            }
                        }
                    }
                }
                let source = client.tasks(&from).map_err(|error| error.to_string());
                let destination = client.tasks(&to).map_err(|error| error.to_string());
                if out
                    .send(Msg::Moved {
                        operation,
                        from,
                        to,
                        ids,
                        result: MoveOutcome {
                            moved,
                            failure,
                            source,
                            destination,
                        },
                    })
                    .is_err()
                {
                    return;
                }
            }
            Cmd::Refresh(generation) => {
                let lists = client.tasklists().map_err(|e| e.to_string());
                let ids: Vec<String> = lists.iter().flatten().map(|l| l.id.clone()).collect();
                if out.send(Msg::Lists(generation, lists)).is_err() {
                    return;
                }
                for id in ids {
                    let tasks = client.tasks(&id).map_err(|e| e.to_string());
                    if out.send(Msg::Tasks(generation, id, tasks)).is_err() {
                        return;
                    }
                }
            }
        }
    }
}

/// Returns whether shutdown should join the worker after draining writes.
fn run(mut term: DefaultTerminal, mut app: App, msgs: Receiver<Msg>) -> std::io::Result<bool> {
    let mut last_poll = Instant::now();
    while !app.quit {
        term.draw(|f| app.draw(f))?;
        // Block on input, but wake often enough to pick up API results.
        if event::poll(TICK)? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    app.on_key(k);
                }
            }
        }
        loop {
            match msgs.try_recv() {
                Ok(msg) => app.on_msg(msg),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    app.worker_disconnected();
                    break;
                }
            }
        }
        if last_poll.elapsed() >= THEME_POLL {
            app.check_theme();
            last_poll = Instant::now();
        }
    }
    Ok(app.quitting && !app.force_quit)
}

/// Ask the terminal to report Shift+Enter and friends (kitty keyboard
/// protocol). Returns whether the flags were pushed, so they can be popped.
fn enable_key_detail() -> bool {
    if !terminal::supports_keyboard_enhancement().unwrap_or(false) {
        return false;
    }
    execute!(
        io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )
    .is_ok()
}

pub fn start() -> auth::Result<()> {
    let paths = Paths::from_env();
    // Reuse the stored token so nothing blocks the first frame; only a first
    // run (or a wiped token) needs the browser, which wants the terminal.
    let auth = match auth::Auth::cached_with_paths(&paths) {
        Some(a) => a,
        None => auth::Auth::new_with_paths(&paths)?,
    };
    let account = auth.fingerprint();
    let (cached, cache_meta, warning) = match cache::load_for(&paths, &account) {
        Ok((cached, meta)) => (cached, meta, None),
        Err(error) => (
            cache::Cache::default(),
            cache::Meta {
                account,
                generation: 0,
            },
            Some(format!("cache ignored: {error}")),
        ),
    };
    let (config, config_warning) = config::load_with_paths(&paths);
    let warning = match (warning, config_warning) {
        (Some(cache), Some(config)) => Some(format!("{cache}; {config}")),
        (warning, None) | (None, warning) => warning,
    };
    let client = Client::new(auth);
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (msg_tx, msg_rx) = mpsc::channel();
    let worker_handle = thread::spawn(move || worker(client, cmd_rx, msg_tx));

    let term = ratatui::init();
    let detailed_keys = enable_key_detail();
    let res = run(
        term,
        App::new_with_storage(
            cmd_tx,
            cached,
            config,
            paths,
            cache_meta,
            warning,
        ),
        msg_rx,
    );
    if detailed_keys {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    ratatui::restore();
    let join_worker = res.as_ref().copied().unwrap_or(true);
    if join_worker && worker_handle.join().is_err() && res.is_ok() {
        return Err("network worker panicked".into());
    }
    res?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_lists(ids: &[&str]) -> (App, Receiver<Cmd>) {
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
        (App::new(tx, cached, Config::default()), rx)
    }

    fn set_tasks(app: &mut App, list_id: &str, tasks: Vec<Task>, selected: &str) {
        app.board.replace_tasks(list_id, tasks);
        app.rebuild_column(list_id);
        let column = app
            .columns
            .iter_mut()
            .find(|column| column.list.id == list_id)
            .unwrap();
        column.select_id(Some(selected.into()));
    }

    fn task(id: &str, parent: Option<&str>, pos: &str, done: bool) -> Task {
        Task {
            id: id.into(),
            parent: parent.map(Into::into),
            position: pos.into(),
            status: if done { "completed" } else { "needsAction" }.into(),
            ..Default::default()
        }
    }

    fn edit(buf: &str) -> Edit {
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

    fn new_edit(list_id: &str, selection: Option<&str>, buf: &str) -> Edit {
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

    fn pending(app: &mut App, context: MutationContext) -> u64 {
        app.pending_operation = Some(PendingOperation { id: 1, context });
        1
    }

    #[test]
    fn edits_title_around_the_cursor() {
        let mut e = edit("Buy milk");
        e.insert('!');
        assert_eq!(e.buf, "Buy milk!");
        e.backspace();
        e.kill_word();
        assert_eq!((e.buf.as_str(), e.cursor), ("Buy ", 4));
        e.insert('é');
        e.left();
        e.delete();
        assert_eq!(e.buf, "Buy ");
        e.kill_to_start();
        assert_eq!((e.buf.as_str(), e.cursor), ("", 0));

        let mut e = edit("e\u{301}x");
        e.left();
        assert_eq!(e.cursor, "e\u{301}".len());
        e.left();
        assert_eq!(e.cursor, 0);
    }

    #[test]
    fn a_failed_list_sync_keeps_the_board_read_only() {
        let (mut app, _cmds) = app_with_lists(&["a", "b"]);
        let generation = app.refresh_generation;
        let lists = app.columns.iter().map(|c| c.list.clone()).collect();

        app.on_msg(Msg::Lists(generation, Ok(lists)));
        app.on_msg(Msg::Tasks(
            generation,
            "a".into(),
            Ok(vec![task("a-fresh", None, "1", false)]),
        ));
        app.on_msg(Msg::Tasks(generation, "b".into(), Err("offline".into())));

        assert_ne!(app.sync, SyncState::Ready, "a partial sync must not enable writes");
        assert!(!app.writable());
        assert_eq!(app.columns[1].rows[0].task.id, "b-cached");
    }

    #[test]
    fn a_failed_mutation_enters_read_only_recovery() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        let removed = app.board.remove_tree("a", "a-cached");
        app.rebuild_column("a");
        let operation = pending(
            &mut app,
            MutationContext::Delete {
                list_id: "a".into(),
                task_id: "a-cached".into(),
                removed,
            },
        );

        app.on_msg(Msg::Deleted {
            operation,
            list_id: "a".into(),
            task_id: "a-cached".into(),
            result: Err("offline".into()),
        });

        assert_ne!(app.sync, SyncState::Ready);
        assert_eq!(app.sync, SyncState::Refreshing);
        assert!(app.pending_operation.is_none());
        assert!(app.board.task("a", "a-cached").is_some());
        assert_eq!(app.columns[0].selected_id.as_deref(), Some("a-cached"));
        assert!(app.error.as_deref().is_some_and(|e| e.contains("delete failed")));
        assert!(matches!(cmds.recv().unwrap(), Cmd::Refresh(_)));
    }

    #[test]
    fn a_failed_create_uses_the_same_read_only_recovery() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        let operation = pending(
            &mut app,
            MutationContext::Create(new_edit("a", Some("a-cached"), "draft")),
        );

        app.on_msg(Msg::Created {
            operation,
            list_id: "a".into(),
            selection: Some("a-cached".into()),
            result: Err("offline".into()),
        });

        assert_ne!(app.sync, SyncState::Ready);
        assert_eq!(app.sync, SyncState::Refreshing);
        assert!(app.error.as_deref().is_some_and(|e| e.contains("create failed")));
        assert!(matches!(cmds.recv().unwrap(), Cmd::Refresh(_)));
    }

    #[test]
    fn a_disconnected_worker_does_not_create_a_pending_save() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        drop(cmds);
        app.sync = SyncState::Ready;

        app.send_patch(
            "a".into(),
            "a-cached".into(),
            TaskPatch {
                title: Some("changed".into()),
                ..Default::default()
            },
            task("a-cached", None, "1", false),
        );

        assert!(app.pending_operation.is_none());
        assert_eq!(app.sync, SyncState::Failed);
        assert!(app.error.as_deref().is_some_and(|e| e.contains("worker stopped")));
    }

    #[test]
    fn only_one_mutation_can_be_in_flight() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;

        app.send_patch(
            "a".into(),
            "one".into(),
            TaskPatch {
                title: Some("one".into()),
                ..Default::default()
            },
            task("one", None, "1", false),
        );
        app.send_patch(
            "a".into(),
            "two".into(),
            TaskPatch {
                title: Some("two".into()),
                ..Default::default()
            },
            task("two", None, "2", false),
        );

        assert!(app.pending_operation.is_some());
        let patches = cmds
            .try_iter()
            .filter(|cmd| matches!(cmd, Cmd::Patch { .. }))
            .count();
        assert_eq!(patches, 1);
    }

    #[test]
    fn navigation_remains_available_while_mutations_are_blocked() {
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

        app.send_patch(
            "a".into(),
            "a-one".into(),
            TaskPatch {
                title: Some("changed".into()),
                ..Default::default()
            },
            task("a-one", None, "1", false),
        );
        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

        assert_eq!(app.selected_task().map(|row| row.task.id.as_str()), Some("a-two"));
        assert!(app.error.as_deref().is_some_and(|error| error.contains("current save")));
        assert_eq!(
            cmds.try_iter()
                .filter(|command| matches!(command, Cmd::Patch { .. }))
                .count(),
            1
        );
    }

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
        assert_eq!(app.columns[0].state.selected(), Some(1));
    }

    #[test]
    fn a_failed_patch_rolls_back_the_canonical_task() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        app.sync = SyncState::Ready;
        let before = app.board.task("a", "a-cached").unwrap().clone();
        app.board.task_mut("a", "a-cached").unwrap().title = "optimistic".into();
        app.rebuild_column("a");
        let operation = pending(
            &mut app,
            MutationContext::Patch {
                list_id: "a".into(),
                task_id: "a-cached".into(),
                before: Box::new(before),
            },
        );

        app.on_msg(Msg::Updated {
            operation,
            list_id: "a".into(),
            task_id: "a-cached".into(),
            result: Err("offline".into()),
        });

        assert_ne!(
            app.board.task("a", "a-cached").unwrap().title,
            "optimistic"
        );
        assert_eq!(
            app.columns[0].rows[0].task.title,
            app.board.task("a", "a-cached").unwrap().title
        );
    }

    #[test]
    fn a_response_for_another_operation_is_ignored() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        app.sync = SyncState::Ready;
        pending(
            &mut app,
            MutationContext::Patch {
                list_id: "a".into(),
                task_id: "a-cached".into(),
                before: Box::new(task("a-cached", None, "1", false)),
            },
        );

        app.on_msg(Msg::Updated {
            operation: 99,
            list_id: "a".into(),
            task_id: "a-cached".into(),
            result: Ok(Task {
                title: "wrong response".into(),
                ..task("a-cached", None, "1", false)
            }),
        });

        assert_eq!(app.pending_operation.as_ref().map(|pending| pending.id), Some(1));
        assert_ne!(app.columns[0].rows[0].task.title, "wrong response");
    }

    #[test]
    fn a_failed_create_restores_the_exact_draft() {
        let (mut app, cmds) = app_with_lists(&["a", "b"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        app.start_create(false);
        let edit = app.edit_mut().unwrap();
        edit.buf = "keep this draft".into();
        edit.cursor = edit.buf.len();

        app.commit_edit();
        let _ = cmds.recv().unwrap(); // create command
        app.focus = 1; // navigation remains available while the create is pending
        let operation = app.pending_operation.as_ref().unwrap().id;
        app.on_msg(Msg::Created {
            operation,
            list_id: "a".into(),
            selection: Some("a-cached".into()),
            result: Err("offline".into()),
        });

        assert_eq!(app.edit().map(|e| e.buf.as_str()), Some("keep this draft"));
        assert!(app.is_creating());
        assert_eq!(app.columns[app.focus].list.id, "a");
        let _ = cmds.recv().unwrap(); // recovery refresh
        app.commit_edit();
        assert!(cmds.try_recv().is_err(), "draft must wait for recovery sync");
        assert_eq!(app.edit().map(|e| e.buf.as_str()), Some("keep this draft"));
    }

    #[test]
    fn pending_mutations_prevent_cache_snapshots() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        app.sync = SyncState::Ready;
        pending(
            &mut app,
            MutationContext::Patch {
                list_id: "a".into(),
                task_id: "a-cached".into(),
                before: Box::new(task("a-cached", None, "1", false)),
            },
        );

        assert!(!app.save_cache());
    }

    #[test]
    fn an_old_refresh_cannot_complete_a_newer_one() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let first = match cmds.recv().unwrap() {
            Cmd::Refresh(generation) => generation,
            _ => panic!("expected startup refresh"),
        };

        app.refresh();
        let _ = cmds.recv().unwrap(); // second refresh
        app.on_msg(Msg::Lists(first, Ok(Vec::new()))); // late response from the first

        assert_ne!(app.sync, SyncState::Ready);
        assert_eq!(app.columns.len(), 1);
        assert_eq!(app.columns[0].list.id, "a");
    }

    #[test]
    fn a_delayed_update_does_not_hide_another_tasks_editor() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        set_tasks(
            &mut app,
            "a",
            vec![
                task("a-one", None, "1", false),
                task("a-two", None, "2", false),
            ],
            "a-two",
        );
        app.interaction = Interaction::Editing(Edit {
            target: EditTarget::Existing {
                list_id: "a".into(),
                task_id: "a-two".into(),
                field: FieldId::Title,
            },
            buf: "editing two".into(),
            cursor: 11,
        });
        let operation = pending(
            &mut app,
            MutationContext::Patch {
                list_id: "a".into(),
                task_id: "a-one".into(),
                before: Box::new(task("a-one", None, "1", false)),
            },
        );

        app.on_msg(Msg::Updated {
            operation,
            list_id: "a".into(),
            task_id: "a-one".into(),
            result: Ok(Task {
                title: "updated one".into(),
                ..task("a-one", None, "1", false)
            }),
        });

        assert_eq!(app.selected_task().map(|r| r.task.id.as_str()), Some("a-two"));
        assert_eq!(
            app.edit()
                .and_then(Edit::existing)
                .map(|(_, task_id, _)| task_id),
            Some("a-two")
        );
    }

    #[test]
    fn a_delayed_create_does_not_hide_another_tasks_editor() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        set_tasks(
            &mut app,
            "a",
            vec![
                task("a-one", None, "1", false),
                task("a-two", None, "2", false),
            ],
            "a-two",
        );
        app.interaction = Interaction::Editing(Edit {
            target: EditTarget::Existing {
                list_id: "a".into(),
                task_id: "a-two".into(),
                field: FieldId::Title,
            },
            buf: "editing two".into(),
            cursor: 11,
        });
        let operation = pending(
            &mut app,
            MutationContext::Create(new_edit("a", Some("a-one"), "new task")),
        );

        app.on_msg(Msg::Created {
            operation,
            list_id: "a".into(),
            selection: Some("a-one".into()),
            result: Ok(Task {
                id: "new".into(),
                title: "new task".into(),
                status: "needsAction".into(),
                position: "0".into(),
                ..Default::default()
            }),
        });

        assert_eq!(app.selected_task().map(|r| r.task.id.as_str()), Some("a-two"));
        assert_eq!(
            app.edit()
                .and_then(Edit::existing)
                .map(|(_, task_id, _)| task_id),
            Some("a-two")
        );
    }

    #[test]
    fn quit_waits_for_writes_and_a_second_quit_forces_it() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        pending(
            &mut app,
            MutationContext::Patch {
                list_id: "a".into(),
                task_id: "a-cached".into(),
                before: Box::new(task("a-cached", None, "1", false)),
            },
        );

        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!app.quit, "the first quit must wait for the pending write");

        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.quit, "the second quit is the explicit force-quit path");
    }

    #[test]
    fn a_waiting_quit_finishes_when_the_last_write_returns() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        let operation = pending(
            &mut app,
            MutationContext::Create(new_edit("a", Some("a-cached"), "draft")),
        );

        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!app.quit);
        app.on_msg(Msg::Created {
            operation,
            list_id: "a".into(),
            selection: Some("a-cached".into()),
            result: Err("offline".into()),
        });

        assert!(app.pending_operation.is_none());
        assert!(app.quit);
    }

    #[test]
    fn worker_disconnect_does_not_hang_a_waiting_quit() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        pending(
            &mut app,
            MutationContext::Patch {
                list_id: "a".into(),
                task_id: "a-cached".into(),
                before: Box::new(task("a-cached", None, "1", false)),
            },
        );
        app.quitting = true;

        app.worker_disconnected();

        assert!(app.pending_operation.is_none());
        assert!(app.quit);
        assert_eq!(app.sync, SyncState::Failed);
        assert!(app.error.as_deref().is_some_and(|e| e.contains("worker stopped")));
    }

    #[test]
    fn view_scrolls_to_keep_the_cursor_visible() {
        let mut e = edit("abcdefgh");
        assert_eq!(e.view(4), ("fgh", 3));
        e.cursor = 0;
        assert_eq!(e.view(4), ("abcd", 0));
        assert_eq!(edit("").view(4), ("", 0));
        assert_eq!(edit("任务a").view(3), ("a", 1));
    }

    #[test]
    fn visual_line_navigation_uses_the_same_wrapping_as_rendering() {
        let mut e = edit("abcdef");
        e.cursor = 2;
        e.step_visual_line(3, true);
        assert_eq!(e.cursor, 5);
        e.step_visual_line(3, false);
        assert_eq!(e.cursor, 2);
    }

    #[test]
    fn orders_open_then_done_with_subtasks_under_parents() {
        let rows = order(vec![
            task("done", None, "0", true),
            task("b", None, "2", false),
            task("a1", Some("a"), "1", false),
            task("a", None, "1", false),
            task("orphan", Some("gone"), "3", false),
        ]);
        let got: Vec<(&str, u8)> = rows.iter().map(|r| (r.task.id.as_str(), r.depth)).collect();
        assert_eq!(got, [("a", 0), ("a1", 1), ("b", 0), ("orphan", 0), ("done", 0)]);
    }

    #[test]
    fn urls_in_finds_http_and_https_and_skips_the_rest() {
        assert_eq!(urls_in(""), Vec::<String>::new());
        assert_eq!(urls_in("no links here"), Vec::<String>::new());
        assert_eq!(
            urls_in("see https://example.com/a"),
            vec!["https://example.com/a"]
        );
        assert_eq!(
            urls_in("http://a.test https://b.test/x"),
            vec!["http://a.test", "https://b.test/x"]
        );
        assert_eq!(
            urls_in("read [PR](https://github.com/org/repo/pull/1)."),
            vec!["https://github.com/org/repo/pull/1"]
        );
        assert_eq!(urls_in("visit https://ex.com."), vec!["https://ex.com"]);
        assert_eq!(urls_in("www.example.com"), Vec::<String>::new());
        assert_eq!(
            urls_in("https://dup.test then https://dup.test again"),
            vec!["https://dup.test"]
        );
    }

    #[test]
    fn task_links_lists_attached_links_then_notes_urls() {
        use crate::api::Link;
        let link = |d: &str, u: &str| Link { description: d.into(), link: u.into() };
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
    fn task_titles_wrap_only_when_asked() {
        let t = Theme::default();
        let today = Local::now().date_naive();
        let long = Task {
            title: "Extend irrigation to the raised beds before the frost".into(),
            ..task("a", None, "1", false)
        };
        let rows = order(vec![long.clone(), Task { parent: Some("a".into()), ..long }]);
        // Clipped: one line per task, however long the title is.
        assert_eq!(task_item(&rows[0], &t, today, None).height(), 1);
        // Wrapped: a 53-character title needs several lines in a 20-wide column.
        let top = task_item(&rows[0], &t, today, Some(20)).height();
        assert!(top > 2, "expected the title to wrap, got {top} line(s)");
        // A subtask is indented, so it has less room and needs at least as many.
        assert!(task_item(&rows[1], &t, today, Some(20)).height() >= top);
        // A width narrower than the indent and marker still renders.
        assert!(task_item(&rows[1], &t, today, Some(1)).height() >= 1);
    }

    #[test]
    fn clean_swaps_control_characters_without_moving_offsets() {
        assert!(matches!(clean("plain text"), Cow::Borrowed(_)));
        // Newlines survive: wrap() splits lines on them.
        assert!(matches!(clean("two\nlines"), Cow::Borrowed(_)));
        assert_eq!(clean("print \r and bring"), "print   and bring");
        assert_eq!(clean("bell\u{7}tab\u{8}"), "bell tab ");
        // Byte offsets have to survive, or wrap()/cursor_rc() slice mid-char.
        for raw in ["print \r and", "a\u{7}b\u{1b}c", "\r\n\r\n", "héllo\rwörld"] {
            assert_eq!(clean(raw).len(), raw.len(), "{raw:?}");
        }
    }

    #[test]
    fn a_yank_carries_the_links_and_drops_the_bookkeeping() {
        let task = Task {
            id: "t1".into(),
            title: "Check In For Yearly Checkup".into(),
            notes: Some("Need my vax\r\n".into()),
            status: "needsAction".into(),
            due: Some("2026-09-25T00:00:00.000Z".into()),
            parent: Some("p9".into()),
            links: vec![crate::api::Link {
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
    fn ellipsize_cuts_on_character_boundaries() {
        assert_eq!(ellipsize("short", 10), "short");
        assert_eq!(ellipsize("exactfit", 8), "exactfit");
        assert_eq!(ellipsize("truncate me", 8), "truncat…");
        assert_eq!(ellipsize("héllo wörld", 6), "héllo…");
        assert_eq!(ellipsize("abc", 0), "…");
    }

    #[test]
    fn cursor_rc_places_the_cursor_at_the_end_of_a_wrapped_line() {
        let text = "aaa\nbbb";
        let segs = wrap(text, 10);
        assert_eq!(cursor_rc(&segs, text, 1), (0, 1));
        // End of a line that is not the last one: no segment contains it.
        assert_eq!(cursor_rc(&segs, text, 3), (0, 3));
        assert_eq!(cursor_rc(&segs, text, 4), (1, 0));
        assert_eq!(cursor_rc(&segs, text, 7), (1, 3));

        // A soft wrap shares the boundary, so the cursor belongs to the next row.
        let soft = "abcdefgh";
        let segs = wrap(soft, 4);
        assert_eq!(cursor_rc(&segs, soft, 4), (1, 0));

        // Trailing newline, and an empty buffer.
        let trail = "ab\n";
        let segs = wrap(trail, 10);
        assert_eq!(cursor_rc(&segs, trail, 3), (1, 0));
        assert_eq!(cursor_rc(&wrap("", 10), "", 0), (0, 0));

        let wide = "任务ab";
        let segments = wrap(wide, 3);
        assert_eq!(
            segments
                .iter()
                .map(|(start, end)| &wide[*start..*end])
                .collect::<Vec<_>>(),
            ["任", "务a", "b"]
        );
        assert_eq!(cursor_rc(&segments, wide, "任务".len()), (1, 2));
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
        let got: Vec<&str> = other_lists(&lists, "b").iter().map(|l| l.id.as_str()).collect();
        assert_eq!(got, ["a", "c"]);
        assert!(other_lists(&lists, "missing").iter().map(|l| l.id.as_str()).eq(["a", "b", "c"]));
    }

    #[test]
    fn move_ids_are_the_task_and_its_subtasks() {
        let rows = order(vec![
            task("a", None, "1", false),
            task("a1", Some("a"), "1", false),
            task("b", None, "2", false),
        ]);
        assert_eq!(move_ids(&rows, "a"), ["a", "a1"]);
        assert_eq!(move_ids(&rows, "a1"), ["a1"]);
        assert!(move_ids(&rows, "gone").is_empty());
    }

    #[test]
    fn move_requests_preserve_subtask_sibling_order() {
        let ids = vec!["parent".into(), "first".into(), "second".into()];
        let requests = move_requests("destination", &ids);

        assert_eq!(requests[0].1.parent, None);
        assert_eq!(requests[0].1.previous, None);
        assert_eq!(requests[1].1.parent.as_deref(), Some("parent"));
        assert_eq!(requests[1].1.previous, None);
        assert_eq!(requests[2].1.parent.as_deref(), Some("parent"));
        assert_eq!(requests[2].1.previous.as_deref(), Some("first"));
    }

    #[test]
    fn partial_move_applies_authoritative_lists_and_reports_the_failed_id() {
        let (mut app, cmds) = app_with_lists(&["a", "b"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        let parent = task("parent", None, "1", false);
        let child = task("child", Some("parent"), "2", false);
        let there = task("there", None, "1", false);
        set_tasks(
            &mut app,
            "a",
            vec![parent.clone(), child.clone()],
            "parent",
        );
        set_tasks(&mut app, "b", vec![there.clone()], "there");
        let operation = pending(
            &mut app,
            MutationContext::Move {
                from: "a".into(),
                to: "b".into(),
                ids: vec!["parent".into(), "child".into()],
                source_before: vec![parent.clone(), child.clone()],
                destination_before: vec![there.clone()],
            },
        );
        let moved_parent = Task {
            position: "9".into(),
            ..parent
        };

        app.on_msg(Msg::Moved {
            operation,
            from: "a".into(),
            to: "b".into(),
            ids: vec!["parent".into(), "child".into()],
            result: MoveOutcome {
                moved: vec![moved_parent.clone()],
                failure: Some(MoveFailure {
                    task_id: "child".into(),
                    error: "cannot move assigned task".into(),
                }),
                source: Ok(vec![child]),
                destination: Ok(vec![there, moved_parent]),
            },
        });

        assert!(app.board.task("a", "parent").is_none());
        assert_eq!(
            app.board.task("b", "parent").map(|task| task.position.as_str()),
            Some("9")
        );
        assert!(app.error.as_deref().is_some_and(|error| {
            error.contains("partially completed") && error.contains("child")
        }));
        assert!(matches!(cmds.recv().unwrap(), Cmd::Refresh(_)));
    }

    #[test]
    fn assigned_tasks_cannot_move_or_edit_notes() {
        let (mut app, _cmds) = app_with_lists(&["a", "b"]);
        app.sync = SyncState::Ready;
        let assigned = Task {
            assignment_info: Some(crate::api::AssignmentInfo {
                link_to_task: "https://docs.google.com/document/d/1".into(),
                surface_type: "DOCUMENT".into(),
            }),
            ..task("assigned", None, "1", false)
        };
        set_tasks(&mut app, "a", vec![assigned], "assigned");

        app.start_move();
        assert!(matches!(app.interaction, Interaction::Idle));
        assert!(app.error.as_deref().is_some_and(|error| error.contains("cannot move")));

        app.error = None;
        app.shelf = true;
        app.field = FieldId::Notes;
        app.start_edit();
        assert!(matches!(app.interaction, Interaction::Idle));
        assert!(app.error.as_deref().is_some_and(|error| error.contains("cannot have notes")));
    }

    #[test]
    fn help_keys_lists_the_board_commands() {
        let keys: Vec<&str> = help_keys().iter().map(|(k, _)| *k).collect();
        for need in ["h / l", "j / k", "Enter", "Space", "c", "C", "i", "d", "b", "m", "?", "q"] {
            assert!(keys.iter().any(|k| k.contains(need)), "missing {need} in {keys:?}");
        }
        assert!(!help_keys().is_empty());
        assert!(help_keys().iter().all(|(k, d)| !k.is_empty() && !d.is_empty()));
    }

    #[test]
    fn modified_shortcuts_do_not_trigger_plain_commands() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        app.sync = SyncState::Ready;

        app.on_key(KeyEvent::new(
            KeyCode::Char('d'),
            KeyModifiers::CONTROL,
        ));
        assert!(matches!(app.interaction, Interaction::Idle));

        app.on_key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        ));
        assert!(app.quit);
    }

    #[test]
    fn modal_rect_never_exceeds_the_terminal() {
        let area = Rect::new(3, 4, 12, 2);
        let modal = modal_rect(area, 80, 30);
        assert_eq!(modal, area);
    }

    #[test]
    fn subtask_parent_is_the_row_or_its_own_parent() {
        let rows = order(vec![
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

#[cfg(test)]
mod render_tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    /// Render a board of `(list title, task titles)` and return the screen.
    fn screen(w: u16, h: u16, lists: &[(&str, &[&str])], wrap_titles: bool) -> Vec<String> {
        let (tx, _rx) = mpsc::channel();
        let cached = cache::Cache {
            lists: lists
                .iter()
                .enumerate()
                .map(|(i, (name, titles))| cache::CachedList {
                    list: TaskList {
                        id: format!("l{i}"),
                        title: (*name).into(),
                        ..Default::default()
                    },
                    tasks: titles
                        .iter()
                        .enumerate()
                        .map(|(j, title)| Task {
                            id: format!("l{i}t{j}"),
                            title: (*title).into(),
                            position: format!("{j:09}"),
                            status: "needsAction".into(),
                            ..Default::default()
                        })
                        .collect(),
                })
                .collect(),
        };
        let mut app = App::new(tx, cached, Config { wrap_titles });
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn a_wrapped_line_never_starts_on_a_stray_space() {
        // Breaking on a run of spaces hands the next line a leading one, which
        // shows up as a one-column indent under the rest of the title.
        let title = "aaaaaaaaaaaaaaaaaaaaaaaa  gamma";
        // 30 wide puts the break between the two spaces.
        let rows = screen(30, 8, &[("One", &[title])], true);
        let col = |needle: &str| {
            rows.iter()
                .find_map(|r| r.find(needle).map(|b| r[..b].chars().count()))
                .unwrap_or_else(|| panic!("{needle} is not on screen: {rows:#?}"))
        };
        assert_eq!(col("gamma"), col("aaaa"));
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(row.chars().count(), 30, "row {i} is {row:?}");
        }
    }

    /// A control character reaching a cell is not cosmetic: printing `\r`
    /// walks the terminal cursor back to column 0 and shifts the rest of the
    /// row. Clipping hid them past a column's edge; wrapping does not.
    #[test]
    fn control_characters_never_reach_the_screen() {
        let rows = screen(
            34,
            14,
            &[("One", &["print \r and bring with you to the hospital", "bell\u{7}here"]),
              ("Two\r", &["fine"])],
            true,
        );
        for (y, row) in rows.iter().enumerate() {
            for c in row.chars() {
                assert!(!c.is_ascii_control(), "{c:?} on row {y}: {row:?}");
            }
            assert_eq!(row.chars().count(), 34, "row {y} is {row:?}");
        }
        assert!(rows.iter().any(|r| r.contains("and bring")), "{rows:#?}");
    }

    #[test]
    fn every_row_of_the_board_is_the_full_width() {
        let long = "Complete Birth Certificate Worksheet as much as possible, print and bring";
        let rows = screen(
            62,
            20,
            &[("One", &["Extend irrigation to the outer plants", "Baby go bag prep"]),
              ("Two", &[long, "Irrigation for box"])],
            true,
        );
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(row.chars().count(), 62, "row {i} is {row:?}");
        }
    }

    #[test]
    fn shelf_keeps_notes_and_updated_visible_with_many_links() {
        let (tx, _rx) = mpsc::channel();
        let task = Task {
            id: "task".into(),
            title: "Task".into(),
            notes: Some("Important notes".into()),
            status: "needsAction".into(),
            links: (0..12)
                .map(|index| crate::api::Link {
                    description: format!("Link {index}"),
                    link: format!("https://example.test/{index}"),
                })
                .collect(),
            ..Default::default()
        };
        let cached = cache::Cache {
            lists: vec![cache::CachedList {
                list: TaskList {
                    id: "list".into(),
                    title: "List".into(),
                    ..Default::default()
                },
                tasks: vec![task],
            }],
        };
        let mut app = App::new(tx, cached, Config::default());
        app.shelf = true;
        let mut terminal = Terminal::new(TestBackend::new(90, 18)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let screen: String = (0..18)
            .flat_map(|y| {
                (0..90)
                    .map(move |x| buffer[(x, y)].symbol())
                    .chain(std::iter::once("\n"))
            })
            .collect();

        assert!(screen.contains("Notes"), "{screen}");
        assert!(screen.contains("Important notes"), "{screen}");
        assert!(screen.contains("Updated"), "{screen}");
        assert!(screen.contains("More"), "{screen}");
    }

    #[test]
    fn narrow_terminal_uses_a_visible_full_width_editor() {
        let (tx, _rx) = mpsc::channel();
        let cached = cache::Cache {
            lists: vec![cache::CachedList {
                list: TaskList {
                    id: "list".into(),
                    title: "List".into(),
                    ..Default::default()
                },
                tasks: Vec::new(),
            }],
        };
        let mut app = App::new(tx, cached, Config::default());
        app.sync = SyncState::Ready;
        app.start_create(false);
        let mut terminal = Terminal::new(TestBackend::new(36, 10)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();

        assert!(app.edit_width > 0);
        let buffer = terminal.backend().buffer();
        let screen: String = (0..10)
            .flat_map(|y| {
                (0..36)
                    .map(move |x| buffer[(x, y)].symbol())
                    .chain(std::iter::once("\n"))
            })
            .collect();
        assert!(screen.contains("New task"), "{screen}");
    }
}


