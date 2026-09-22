use std::{
    collections::HashSet,
    io,
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use chrono::{DateTime, Datelike, Local, NaiveDate};
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
        Block, BorderType, HighlightSpacing, List, ListItem, ListState, Padding, Paragraph,
    },
    DefaultTerminal, Frame,
};

use serde_json::{json, Value};

use crate::api::{Client, Task, TaskList};
use crate::auth;
use crate::cache;
use crate::theme::{self, Theme};

const THEME_POLL: Duration = Duration::from_millis(500);
const TICK: Duration = Duration::from_millis(100);
const COL_MIN: u16 = 30;
const COL_MAX: u16 = 56;
const SHELF_W: u16 = 52;
const READ_ONLY: &str = "Syncing with Google — read-only for a moment";

/// Requests to the network thread.
enum Cmd {
    Refresh,
    Patch { list_id: String, task_id: String, fields: Value },
    Delete { list_id: String, task_id: String },
    Create { list_id: String, title: String },
    Move { from: String, to: String, ids: Vec<String> },
}

/// Results from the network thread.
enum Msg {
    Lists(Result<Vec<TaskList>, String>),
    Tasks(String, Result<Vec<Task>, String>),
    Updated(String, Result<Task, String>),
    Deleted(Result<(), String>),
    Created(String, Result<Task, String>),
    Moved(Result<(), String>),
}

/// A delete waiting on a y/n answer in the status bar.
struct Pending {
    list_id: String,
    task_id: String,
    prompt: String,
}

/// Overlay listing URLs found in the selected task's notes.
struct Browse {
    urls: Vec<String>,
    state: ListState,
}

/// Overlay listing destination lists for a move.
struct MovePick {
    lists: Vec<TaskList>,
    state: ListState,
}

/// Overlay listing keyboard commands.
struct Help {
    state: ListState,
}

/// The editable fields of the shelf.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldId {
    Title,
    Notes,
}

/// An in-progress edit. `cursor` is a byte offset into `buf`.
struct Edit {
    list_id: String,
    task_id: String,
    field: FieldId,
    buf: String,
    cursor: usize,
}

impl Edit {
    fn insert(&mut self, c: char) {
        self.buf.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    fn left(&mut self) {
        if let Some((i, _)) = self.buf[..self.cursor].char_indices().next_back() {
            self.cursor = i;
        }
    }

    fn right(&mut self) {
        if let Some(c) = self.buf[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }

    fn backspace(&mut self) {
        let end = self.cursor;
        self.left();
        self.buf.replace_range(self.cursor..end, "");
    }

    fn delete(&mut self) {
        if let Some(c) = self.buf[self.cursor..].chars().next() {
            let end = self.cursor + c.len_utf8();
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

    /// Move a logical line up or down, keeping the column where possible.
    fn step_line(&mut self, down: bool) {
        let start = self.line_start();
        let col = self.buf[start..self.cursor].chars().count();
        let target = if down {
            let end = self.line_end();
            if end == self.buf.len() {
                self.cursor = end;
                return;
            }
            end + 1
        } else {
            if start == 0 {
                self.cursor = 0;
                return;
            }
            self.buf[..start - 1].rfind('\n').map_or(0, |i| i + 1)
        };
        let line = &self.buf[target..];
        let len = line.find('\n').unwrap_or(line.len());
        self.cursor = target
            + line[..len]
                .char_indices()
                .nth(col)
                .map_or(len, |(i, _)| i);
    }

    /// The slice that fits `width` columns with the cursor in view, and the
    /// cursor's offset within it.
    fn view(&self, width: usize) -> (&str, usize) {
        let width = width.max(1);
        let chars: Vec<usize> = self.buf.char_indices().map(|(i, _)| i).collect();
        let cur = chars.iter().position(|i| *i >= self.cursor).unwrap_or(chars.len());
        let start = cur.saturating_sub(width - 1);
        let end = (start + width).min(chars.len());
        let from = chars.get(start).copied().unwrap_or(self.buf.len());
        let to = chars.get(end).copied().unwrap_or(self.buf.len());
        (&self.buf[from..to], cur - start)
    }
}

struct Row {
    task: Task,
    depth: u8,
}

struct Column {
    list: TaskList,
    rows: Vec<Row>,
    loaded: bool,
    state: ListState,
}

impl Column {
    fn selected(&self) -> Option<&Row> {
        self.state.selected().and_then(|i| self.rows.get(i))
    }
}

struct App {
    theme: Theme,
    fingerprint: theme::Fingerprint,
    transparent: bool,
    columns: Vec<Column>,
    focus: usize,
    /// Index of the leftmost visible column.
    scroll: usize,
    shelf: bool,
    /// Which shelf field the keyboard is on.
    field: FieldId,
    /// The shelf is holding a not-yet-created task.
    creating: bool,
    edit: Option<Edit>,
    pending: Option<Pending>,
    browse: Option<Browse>,
    move_to: Option<MovePick>,
    help: Option<Help>,
    fetching: bool,
    /// False while the board may still be stale: edits are blocked until the
    /// first sync of this run lands.
    synced: bool,
    /// Edits sent to Google that haven't come back yet.
    saving: usize,
    error: Option<String>,
    cmds: Sender<Cmd>,
    quit: bool,
}

impl App {
    fn new(cmds: Sender<Cmd>, cached: cache::Cache) -> Self {
        // Paint the last known board straight away; the sync replaces it.
        let columns = cached
            .lists
            .into_iter()
            .map(|c| Column {
                list: c.list,
                state: ListState::default().with_selected((!c.tasks.is_empty()).then_some(0)),
                rows: order(c.tasks),
                loaded: true,
            })
            .collect();
        let mut app = Self {
            theme: theme::load(),
            fingerprint: theme::fingerprint(),
            transparent: false,
            columns,
            focus: 0,
            scroll: 0,
            shelf: false,
            field: FieldId::Title,
            creating: false,
            edit: None,
            pending: None,
            browse: None,
            move_to: None,
            help: None,
            fetching: false,
            synced: false,
            saving: 0,
            error: None,
            cmds,
            quit: false,
        };
        app.refresh();
        app
    }

    fn refresh(&mut self) {
        self.error = None;
        self.fetching = true;
        self.synced = false;
        let _ = self.cmds.send(Cmd::Refresh);
    }

    /// Editing waits until this run has seen Google's copy of every list.
    fn writable(&mut self) -> bool {
        if !self.synced {
            self.error = Some(READ_ONLY.into());
        }
        self.synced
    }

    fn check_synced(&mut self) {
        if self.fetching || !self.columns.iter().all(|c| c.loaded) {
            return;
        }
        self.synced = true;
        if self.error.as_deref() == Some(READ_ONLY) {
            self.error = None;
        }
        self.save_cache();
    }

    fn save_cache(&self) {
        cache::save(&cache::Cache {
            lists: self
                .columns
                .iter()
                .map(|c| cache::CachedList {
                    list: c.list.clone(),
                    tasks: c.rows.iter().map(|r| r.task.clone()).collect(),
                })
                .collect(),
        });
    }

    fn loading(&self) -> bool {
        self.fetching || self.columns.iter().any(|c| !c.loaded)
    }

    /// Start a new task in the focused list: an empty title input in the shelf.
    fn start_create(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(col) = self.columns.get(self.focus) else { return };
        self.edit = Some(Edit {
            list_id: col.list.id.clone(),
            task_id: String::new(), // no id yet: commit POSTs instead of PATCHing
            field: FieldId::Title,
            buf: String::new(),
            cursor: 0,
        });
        self.creating = true;
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
        self.edit = Some(Edit { list_id, task_id, field: self.field, cursor: buf.len(), buf });
    }

    fn row_mut(&mut self, list_id: &str, task_id: &str) -> Option<&mut Row> {
        self.columns
            .iter_mut()
            .find(|c| c.list.id == list_id)
            .and_then(|c| c.rows.iter_mut().find(|r| r.task.id == task_id))
    }

    fn commit_edit(&mut self) {
        let Some(e) = self.edit.take() else { return };
        if self.creating {
            let title = e.buf.trim().to_string();
            if title.is_empty() {
                self.error = Some("A task needs a title".into());
                self.edit = Some(e);
                return;
            }
            self.creating = false;
            self.error = None;
            self.saving += 1;
            let _ = self.cmds.send(Cmd::Create { list_id: e.list_id, title });
            return;
        }
        // Show the change straight away; Google confirms it a moment later.
        let fields = match e.field {
            FieldId::Title => {
                let title = e.buf.trim().to_string();
                if title.is_empty() {
                    self.error = Some("A task needs a title".into());
                    self.edit = Some(e);
                    return;
                }
                if let Some(row) = self.row_mut(&e.list_id, &e.task_id) {
                    row.task.title = title.clone();
                }
                json!({ "title": title })
            }
            FieldId::Notes => {
                let notes = e.buf.trim_end().to_string();
                if let Some(row) = self.row_mut(&e.list_id, &e.task_id) {
                    row.task.notes = Some(notes.clone()).filter(|n| !n.is_empty());
                }
                json!({ "notes": notes })
            }
        };
        self.error = None;
        self.send_patch(e.list_id, e.task_id, fields);
    }

    /// Tick or untick the selected task.
    fn toggle_done(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(col) = self.columns.get_mut(self.focus) else { return };
        let list_id = col.list.id.clone();
        let Some(i) = col.state.selected() else { return };
        let Some(row) = col.rows.get_mut(i) else { return };
        let task_id = row.task.id.clone();
        // Show the new state now; Google's copy replaces it when it answers.
        let fields = if row.task.is_done() {
            row.task.status = "needsAction".into();
            row.task.completed = None;
            json!({ "status": "needsAction", "completed": null })
        } else {
            row.task.status = "completed".into();
            json!({ "status": "completed" })
        };
        self.send_patch(list_id, task_id, fields);
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
        self.pending = Some(Pending {
            list_id: col.list.id.clone(),
            task_id: row.task.id.clone(),
            prompt: format!("Delete \"{}\"{also}?  y/n", title_of(&row.task)),
        });
    }

    fn confirm_delete(&mut self) {
        let Some(p) = self.pending.take() else { return };
        if let Some(col) = self.columns.iter_mut().find(|c| c.list.id == p.list_id) {
            // Drop the task and its subtasks locally; Google cascades the same way.
            col.rows
                .retain(|r| r.task.id != p.task_id && r.task.parent.as_deref() != Some(p.task_id.as_str()));
            let last = col.rows.len().checked_sub(1);
            let sel = col.state.selected().unwrap_or(0);
            col.state.select(last.map(|l| sel.min(l)));
        }
        self.error = None;
        self.saving += 1;
        let _ = self.cmds.send(Cmd::Delete { list_id: p.list_id, task_id: p.task_id });
    }

    fn send_patch(&mut self, list_id: String, task_id: String, fields: Value) {
        self.saving += 1;
        let _ = self.cmds.send(Cmd::Patch { list_id, task_id, fields });
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
        match msg {
            Msg::Lists(Err(e)) => {
                self.fetching = false;
                self.error = Some(e);
            }
            Msg::Lists(Ok(lists)) => {
                self.fetching = false;
                // Keep existing columns (and their selection) until fresh tasks arrive.
                let focused = self.columns.get(self.focus).map(|c| c.list.id.clone());
                let mut old = std::mem::take(&mut self.columns);
                self.columns = lists
                    .into_iter()
                    .map(|list| match old.iter().position(|c| c.list.id == list.id) {
                        Some(i) => Column { list, loaded: false, ..old.swap_remove(i) },
                        None => Column { list, rows: Vec::new(), loaded: false, state: ListState::default() },
                    })
                    .collect();
                self.focus = focused
                    .and_then(|id| self.columns.iter().position(|c| c.list.id == id))
                    .unwrap_or(0);
                self.check_synced(); // an account with no lists is already in sync
            }
            Msg::Tasks(id, res) => {
                let Some(col) = self.columns.iter_mut().find(|c| c.list.id == id) else { return };
                col.loaded = true;
                match res {
                    Ok(tasks) => {
                        let keep = col.selected().map(|r| r.task.id.clone());
                        let prev = col.state.selected();
                        col.rows = order(tasks);
                        let last = col.rows.len().checked_sub(1);
                        let sel = keep
                            .and_then(|id| col.rows.iter().position(|r| r.task.id == id))
                            .or_else(|| last.map(|l| prev.unwrap_or(0).min(l)));
                        col.state.select(sel);
                    }
                    Err(e) => self.error = Some(format!("{}: {e}", col.list.title)),
                }
                self.check_synced();
            }
            Msg::Created(list_id, res) => {
                self.saving = self.saving.saturating_sub(1);
                match res {
                    Ok(task) => {
                        let Some(col) = self.columns.iter_mut().find(|c| c.list.id == list_id)
                        else {
                            return;
                        };
                        let id = task.id.clone();
                        let mut tasks: Vec<Task> = col.rows.iter().map(|r| r.task.clone()).collect();
                        tasks.push(task);
                        col.rows = order(tasks);
                        if let Some(i) = col.rows.iter().position(|r| r.task.id == id) {
                            col.state.select(Some(i));
                        }
                        self.save_cache();
                    }
                    Err(e) => self.error = Some(format!("create failed: {e}")),
                }
            }
            Msg::Deleted(res) => {
                self.saving = self.saving.saturating_sub(1);
                self.save_cache();
                if let Err(e) = res {
                    // The task is still there, so put the board back.
                    self.error = Some(format!("delete failed: {e}"));
                    self.fetching = true;
                    let _ = self.cmds.send(Cmd::Refresh);
                }
            }
            Msg::Moved(res) => {
                self.saving = self.saving.saturating_sub(1);
                self.save_cache();
                if let Err(e) = res {
                    self.error = Some(format!("move failed: {e}"));
                    self.fetching = true;
                    let _ = self.cmds.send(Cmd::Refresh);
                }
            }
            Msg::Updated(list_id, res) => {
                self.saving = self.saving.saturating_sub(1);
                match res {
                    Ok(task) => {
                        let Some(col) = self.columns.iter_mut().find(|c| c.list.id == list_id)
                        else {
                            return;
                        };
                        // Re-sort: ticking a task moves it down to the completed group.
                        let id = task.id.clone();
                        let tasks = col
                            .rows
                            .iter()
                            .map(|r| if r.task.id == id { task.clone() } else { r.task.clone() })
                            .collect();
                        col.rows = order(tasks);
                        if let Some(i) = col.rows.iter().position(|r| r.task.id == id) {
                            col.state.select(Some(i));
                        }
                        self.save_cache();
                    }
                    Err(e) => {
                        // The optimistic title may be wrong now, so resync.
                        self.error = Some(format!("save failed: {e}"));
                        self.fetching = true;
                        let _ = self.cmds.send(Cmd::Refresh);
                    }
                }
            }
        }
    }

    fn move_task(&mut self, delta: isize) {
        let Some(col) = self.columns.get_mut(self.focus) else { return };
        let Some(last) = col.rows.len().checked_sub(1) else { return };
        let cur = col.state.selected().unwrap_or(0) as isize;
        col.state.select(Some((cur + delta).clamp(0, last as isize) as usize));
    }

    fn selected_task(&self) -> Option<&Row> {
        self.columns.get(self.focus)?.selected()
    }

    fn start_browse(&mut self) {
        let Some(row) = self.selected_task() else { return };
        let urls = urls_in(row.task.notes.as_deref().unwrap_or(""));
        if urls.is_empty() {
            self.error = Some("no links in notes".into());
            return;
        }
        self.error = None;
        self.browse = Some(Browse {
            urls,
            state: ListState::default().with_selected(Some(0)),
        });
    }

    fn move_browse(&mut self, delta: isize) {
        let Some(b) = self.browse.as_mut() else { return };
        let Some(last) = b.urls.len().checked_sub(1) else { return };
        let cur = b.state.selected().unwrap_or(0) as isize;
        b.state.select(Some((cur + delta).clamp(0, last as isize) as usize));
    }

    fn open_browse(&mut self, all: bool) {
        let urls = {
            let Some(b) = self.browse.as_ref() else { return };
            if all {
                b.urls.clone()
            } else {
                let Some(i) = b.state.selected() else { return };
                let Some(url) = b.urls.get(i) else { return };
                vec![url.clone()]
            }
        };
        for url in &urls {
            open_url(url);
        }
        self.browse = None;
    }

    fn start_move(&mut self) {
        if !self.writable() {
            return;
        }
        let Some(col) = self.columns.get(self.focus) else { return };
        if col.selected().is_none() {
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
        self.move_to = Some(MovePick {
            lists,
            state: ListState::default().with_selected(Some(0)),
        });
    }

    fn step_move(&mut self, delta: isize) {
        let Some(m) = self.move_to.as_mut() else { return };
        let Some(last) = m.lists.len().checked_sub(1) else { return };
        let cur = m.state.selected().unwrap_or(0) as isize;
        m.state.select(Some((cur + delta).clamp(0, last as isize) as usize));
    }

    fn confirm_move(&mut self) {
        let dest_id = {
            let Some(m) = self.move_to.as_ref() else { return };
            let Some(i) = m.state.selected() else { return };
            let Some(list) = m.lists.get(i) else { return };
            list.id.clone()
        };
        self.move_to = None;
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
        let id_set: HashSet<String> = ids.iter().cloned().collect();
        let src = &mut self.columns[self.focus];
        let mut taken = Vec::new();
        src.rows.retain(|r| {
            if id_set.contains(&r.task.id) {
                taken.push(r.task.clone());
                false
            } else {
                true
            }
        });
        let last = src.rows.len().checked_sub(1);
        let sel = src.state.selected().unwrap_or(0);
        src.state.select(last.map(|l| sel.min(l)));
        taken.sort_by_key(|t| ids.iter().position(|id| *id == t.id).unwrap_or(usize::MAX));
        for t in &mut taken {
            if t.parent.as_deref().is_some_and(|p| !id_set.contains(p)) {
                t.parent = None;
            }
        }
        let dest = &mut self.columns[dest_idx];
        let mut tasks: Vec<Task> = dest.rows.iter().map(|r| r.task.clone()).collect();
        tasks.extend(taken);
        dest.rows = order(tasks);
        if let Some(i) = dest.rows.iter().position(|r| r.task.id == task_id) {
            dest.state.select(Some(i));
        }
        self.focus = dest_idx;
        self.error = None;
        self.saving += 1;
        let _ = self.cmds.send(Cmd::Move { from: from_id, to: dest_id, ids });
    }

    fn start_help(&mut self) {
        self.help = Some(Help { state: ListState::default().with_selected(Some(0)) });
    }

    fn step_help(&mut self, delta: isize) {
        let Some(h) = self.help.as_mut() else { return };
        let n = help_keys().len();
        let i = h.state.selected().unwrap_or(0) as isize + delta;
        h.state.select(Some(i.clamp(0, n.saturating_sub(1) as isize) as usize));
    }

    fn on_key(&mut self, k: KeyEvent) {
        if self.edit.is_some() {
            self.edit_key(k);
            return;
        }
        if self.pending.is_some() {
            // Only "y" deletes; every other key cancels.
            match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.confirm_delete(),
                _ => self.pending = None,
            }
            return;
        }
        if self.browse.is_some() {
            match k.code {
                KeyCode::Esc | KeyCode::Char('q') => self.browse = None,
                KeyCode::Char('j') | KeyCode::Down => self.move_browse(1),
                KeyCode::Char('k') | KeyCode::Up => self.move_browse(-1),
                KeyCode::Enter if k.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.open_browse(true)
                }
                KeyCode::Enter => self.open_browse(false),
                _ => {}
            }
            return;
        }
        if self.move_to.is_some() {
            match k.code {
                KeyCode::Esc | KeyCode::Char('q') => self.move_to = None,
                KeyCode::Char('j') | KeyCode::Down => self.step_move(1),
                KeyCode::Char('k') | KeyCode::Up => self.step_move(-1),
                KeyCode::Enter => self.confirm_move(),
                _ => {}
            }
            return;
        }
        if self.help.is_some() {
            match k.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => self.help = None,
                KeyCode::Char('j') | KeyCode::Down => self.step_help(1),
                KeyCode::Char('k') | KeyCode::Up => self.step_help(-1),
                _ => {}
            }
            return;
        }
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc if self.shelf => self.shelf = false,
            KeyCode::Esc => self.quit = true,
            KeyCode::Char('h') | KeyCode::Left => self.focus = self.focus.saturating_sub(1),
            KeyCode::Char('l') | KeyCode::Right => {
                self.focus = (self.focus + 1).min(self.columns.len().saturating_sub(1))
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_task(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_task(-1),
            KeyCode::Char('g') | KeyCode::Home => self.move_task(isize::MIN / 2),
            KeyCode::Char('G') | KeyCode::End => self.move_task(isize::MAX / 2),
            KeyCode::Enter => {
                self.shelf = !self.shelf && self.selected_task().is_some();
                self.field = FieldId::Title;
            }
            KeyCode::Tab | KeyCode::BackTab if self.shelf => {
                self.field = match self.field {
                    FieldId::Title => FieldId::Notes,
                    FieldId::Notes => FieldId::Title,
                }
            }
            KeyCode::Char('i') => self.start_edit(),
            KeyCode::Char(' ') => self.toggle_done(),
            KeyCode::Char('d') => self.ask_delete(),
            KeyCode::Char('c') => self.start_create(),
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('t') => self.transparent = !self.transparent,
            KeyCode::Char('b') => self.start_browse(),
            KeyCode::Char('m') => self.start_move(),
            KeyCode::Char('?') => self.start_help(),
            _ => {}
        }
    }

    /// Keys while editing a field. Enter saves, Esc discards; in notes,
    /// Shift+Enter (or Alt+Enter / Ctrl+J where the terminal can't report it)
    /// starts a new line.
    fn edit_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let newline = k.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT);
        let notes = self.edit.as_ref().is_some_and(|e| e.field == FieldId::Notes);
        match k.code {
            KeyCode::Esc => {
                self.edit = None;
                self.creating = false;
                return;
            }
            KeyCode::Enter if !(notes && newline) => {
                self.commit_edit();
                return;
            }
            _ => {}
        }
        let Some(e) = self.edit.as_mut() else { return };
        match k.code {
            KeyCode::Enter => e.insert('\n'),
            KeyCode::Char('j') if ctrl && notes => e.insert('\n'),
            KeyCode::Char('w') if ctrl => e.kill_word(),
            KeyCode::Char('u') if ctrl => e.kill_to_start(),
            KeyCode::Char('a') if ctrl => e.cursor = e.line_start(),
            KeyCode::Char('e') if ctrl => e.cursor = e.line_end(),
            KeyCode::Char(c) if !ctrl => e.insert(c),
            KeyCode::Backspace => e.backspace(),
            KeyCode::Delete => e.delete(),
            KeyCode::Left => e.left(),
            KeyCode::Right => e.right(),
            KeyCode::Up if notes => e.step_line(false),
            KeyCode::Down if notes => e.step_line(true),
            KeyCode::Home => e.cursor = e.line_start(),
            KeyCode::End => e.cursor = e.line_end(),
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame) {
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
        if self.saving > 0 {
            head.push(Span::styled("  saving…", Style::default().fg(t.yellow)));
        } else if self.loading() {
            head.push(Span::styled("  syncing…", Style::default().fg(t.yellow)));
        }
        f.render_widget(Line::from(head), header);

        self.draw_board(f, body, &t);
        self.draw_browse(f, body, &t);
        self.draw_move(f, body, &t);
        self.draw_help(f, body, &t);

        let hints = if self.browse.is_some() {
            " j/k · ⏎ open · ⇧⏎ all · Esc close"
        } else if self.move_to.is_some() {
            " j/k · ⏎ move · Esc close"
        } else if self.help.is_some() {
            " j/k · Esc close"
        } else if self.creating {
            " ⏎ create · Esc cancel"
        } else if self.edit.as_ref().is_some_and(|e| e.field == FieldId::Notes) {
            " ⏎ save · ⇧⏎ new line · Esc cancel · ^W delete word"
        } else if self.edit.is_some() {
            " ⏎ save · Esc cancel · ^W delete word · ^U clear"
        } else if !self.synced {
            " syncing with Google… board is read-only until it lands"
        } else if self.shelf {
            " Tab field · i edit · c new · ␣ done · j/k task · b link · m move · d delete · ? help · q quit"
        } else {
            " h/l list · j/k task · ␣ done · ⏎ details · c new · i edit · b link · m move · d delete · ? help · q quit"
        };
        let status_line = match (&self.pending, &self.error) {
            (Some(p), _) => Line::styled(format!(" {}", p.prompt), Style::default().fg(t.yellow).bold()),
            (None, Some(e)) => Line::styled(format!(" {e}"), Style::default().fg(t.red)),
            (None, None) => Line::styled(hints, Style::default().fg(t.muted)),
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
        if self.selected_task().is_none() && !self.creating {
            self.shelf = false;
            self.edit = None;
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
        let col = &mut self.columns[i];

        let open = col.rows.iter().filter(|r| !r.task.is_done()).count();
        let title_fg = if focused { t.fg } else { t.muted };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(if focused { t.accent } else { t.muted }))
            .title(Line::styled(format!(" {} ", col.list.title), Style::default().fg(title_fg).bold()))
            .title(Line::styled(format!(" {open} "), Style::default().fg(t.muted)).right_aligned())
            .style(Style::default().bg(surface));

        if col.rows.is_empty() {
            let msg = if col.loaded { "No tasks" } else { "Loading…" };
            f.render_widget(Paragraph::new(msg).style(Style::default().fg(t.muted)).block(block), area);
            return;
        }

        let items: Vec<ListItem> = col.rows.iter().map(|r| task_item(r, t, today)).collect();
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
    fn draw_new_shelf(&self, f: &mut Frame, area: Rect, t: &Theme) {
        let col = &self.columns[self.focus];
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(t.green))
            .title(Line::styled(" New task ", Style::default().fg(t.green).bold()))
            .padding(Padding::horizontal(1))
            .style(Style::default().bg(self.bg(t.bg)));
        let inner = block.inner(area);
        f.render_widget(block, area);
        if inner.width < 8 || inner.height < 3 {
            return;
        }
        let w = inner.width as usize - 2;
        let Some(e) = self.edit.as_ref() else { return };
        let (text, offset) = e.view(w);
        let input = Line::styled(
            format!("{text:<w$}"),
            Style::default().fg(t.sel_fg).bg(t.sel_bg).bold(),
        );
        let lines = vec![
            marked(vec![input], true, t).remove(0),
            Line::default(),
            Line::styled(format!("  in {}", col.list.title), Style::default().fg(t.muted)),
        ];
        f.render_widget(Paragraph::new(lines), inner);
        f.set_cursor_position((inner.x + 2 + offset as u16, inner.y));
    }

    fn draw_shelf(&self, f: &mut Frame, area: Rect, t: &Theme) {
        if self.creating {
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
            return;
        }
        // Two columns go to the focus marker.
        let w = inner.width as usize - 2;
        let editing = self
            .edit
            .as_ref()
            .filter(|e| e.task_id == task.id && e.list_id == col.list.id);
        let editing_field = editing.map(|e| e.field);
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
            meta.push(field("Link", Span::styled(text.clone(), Style::default().fg(t.blue)), t));
        }

        // title · gap · meta · gap · "Notes" · notes · gap · updated
        let title_h = match editing_field {
            Some(FieldId::Title) => 1,
            _ => wrap(title_of(task), w).len().min(3) as u16,
        };
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
        let title_lines = match editing.filter(|e| e.field == FieldId::Title) {
            Some(e) => {
                let (text, offset) = e.view(w);
                cursor = Some((title_a.x + 2 + offset as u16, title_a.y));
                vec![Line::styled(format!("{text:<w$}"), input.bold())]
            }
            None => wrap(title, w)
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
        let notes_text = editing.filter(|e| e.field == FieldId::Notes).map_or(stored, |e| &e.buf);
        let rows = notes_a.height as usize;
        let notes_lines = if notes_text.is_empty() && editing_field != Some(FieldId::Notes) {
            vec![Line::styled("(none)", muted)]
        } else {
            let segs = wrap(notes_text, w);
            let mut scroll = 0;
            if let Some(e) = editing.filter(|e| e.field == FieldId::Notes) {
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
                .map(|(s, e)| Line::styled(format!("{:<w$}", &notes_text[s..e]), style))
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
        let Some(browse) = self.browse.as_mut() else { return };
        let width = area.width.min(72).max(24);
        let height = (browse.urls.len() as u16 + 2).clamp(3, area.height.max(3));
        let x = area.x + area.width.saturating_sub(width) / 2;
        let y = area.y + area.height.saturating_sub(height) / 2;
        let rect = Rect::new(x, y, width, height);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(t.accent))
            .title(Line::styled(" Browse Link ", Style::default().fg(t.accent).bold()))
            .style(Style::default().bg(bg));
        let items: Vec<ListItem> = browse
            .urls
            .iter()
            .map(|u| ListItem::new(Line::styled(u.clone(), Style::default().fg(t.blue))))
            .collect();
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::default().fg(t.sel_fg).bg(t.sel_bg).add_modifier(Modifier::BOLD))
            .highlight_symbol("▌")
            .highlight_spacing(HighlightSpacing::Always);
        f.render_stateful_widget(list, rect, &mut browse.state);
    }

    fn draw_move(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let bg = self.bg(t.bg);
        let Some(pick) = self.move_to.as_mut() else { return };
        let width = area.width.min(72).max(24);
        let height = (pick.lists.len() as u16 + 2).clamp(3, area.height.max(3));
        let x = area.x + area.width.saturating_sub(width) / 2;
        let y = area.y + area.height.saturating_sub(height) / 2;
        let rect = Rect::new(x, y, width, height);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(t.accent))
            .title(Line::styled(" Move ", Style::default().fg(t.accent).bold()))
            .style(Style::default().bg(bg));
        let items: Vec<ListItem> = pick
            .lists
            .iter()
            .map(|l| ListItem::new(Line::styled(l.title.clone(), Style::default().fg(t.fg))))
            .collect();
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::default().fg(t.sel_fg).bg(t.sel_bg).add_modifier(Modifier::BOLD))
            .highlight_symbol("▌")
            .highlight_spacing(HighlightSpacing::Always);
        f.render_stateful_widget(list, rect, &mut pick.state);
    }

    fn draw_help(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let bg = self.bg(t.bg);
        let Some(help) = self.help.as_mut() else { return };
        let keys = help_keys();
        let key_w = keys.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        let width = area.width.min(56).max(28);
        let height = (keys.len() as u16 + 2).clamp(3, area.height.max(3));
        let x = area.x + area.width.saturating_sub(width) / 2;
        let y = area.y + area.height.saturating_sub(height) / 2;
        let rect = Rect::new(x, y, width, height);
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
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::default().fg(t.sel_fg).bg(t.sel_bg).add_modifier(Modifier::BOLD))
            .highlight_symbol("▌")
            .highlight_spacing(HighlightSpacing::Always);
        f.render_stateful_widget(list, rect, &mut help.state);
    }
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
        ("i", "edit focused field"),
        ("d", "delete task"),
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

/// http(s) URLs in notes, first-seen order, no duplicates.
fn urls_in(notes: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < notes.len() {
        let rest = &notes[i..];
        let scheme = if rest.starts_with("https://") {
            "https://"
        } else if rest.starts_with("http://") {
            "http://"
        } else {
            i += rest.chars().next().map(char::len_utf8).unwrap_or(1);
            continue;
        };
        let after = &rest[scheme.len()..];
        let end = after.find(char::is_whitespace).unwrap_or(after.len());
        let mut url = format!("{}{}", scheme, &after[..end]);
        while url
            .chars()
            .last()
            .is_some_and(|c| matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '>' | '\'' | '"'))
        {
            url.pop();
        }
        if url.len() > scheme.len() && !out.iter().any(|u| u == &url) {
            out.push(url);
        }
        i += scheme.len() + end;
    }
    out
}

fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let bin = "open";
    #[cfg(not(target_os = "macos"))]
    let bin = "xdg-open";
    let _ = Command::new(bin)
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
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
            if rest.chars().count() <= width {
                out.push((base + start, base + line.len()));
                break;
            }
            let hard = rest.char_indices().nth(width).map_or(rest.len(), |(i, _)| i);
            // Break on the last space that fits, else mid-word.
            let cut = rest[..hard].rfind(' ').map_or(hard, |i| i + 1);
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
        out = (row, text[*s..cursor.min(*e)].chars().count());
    }
    out
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

fn title_of(task: &Task) -> &str {
    if task.title.trim().is_empty() { "(untitled)" } else { &task.title }
}

fn field<'a>(key: &str, value: Span<'a>, t: &Theme) -> Line<'a> {
    Line::from(vec![Span::styled(format!("{key:<11}"), Style::default().fg(t.muted)), value])
}

fn task_item<'a>(r: &'a Row, t: &Theme, today: NaiveDate) -> ListItem<'a> {
    let task = &r.task;
    let indent = "  ".repeat(r.depth as usize);
    let done = task.is_done();
    let (mark, mark_fg, style) = if done {
        ("✓ ", t.green, Style::default().fg(t.muted).add_modifier(Modifier::CROSSED_OUT))
    } else {
        ("○ ", t.muted, Style::default().fg(t.fg))
    };
    let mut title = vec![
        Span::raw(indent.clone()),
        Span::styled(mark, Style::default().fg(mark_fg)),
        Span::styled(title_of(task), style),
    ];
    if task.notes.as_deref().is_some_and(|n| !n.trim().is_empty()) {
        title.push(Span::styled(" ≡", Style::default().fg(t.muted)));
    }
    let mut lines = vec![Line::from(title)];
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

/// Open tasks by position, then completed ones newest first; subtasks sit
/// directly under their parent.
fn order(tasks: Vec<Task>) -> Vec<Row> {
    let mut tasks: Vec<Task> = tasks.into_iter().filter(|t| !t.deleted).collect();
    tasks.sort_by(|a, b| {
        a.is_done().cmp(&b.is_done()).then_with(|| {
            if a.is_done() { b.completed.cmp(&a.completed) } else { a.position.cmp(&b.position) }
        })
    });
    let ids: HashSet<String> = tasks.iter().map(|t| t.id.clone()).collect();
    let (children, top): (Vec<Task>, Vec<Task>) =
        tasks.into_iter().partition(|t| t.parent.as_ref().is_some_and(|p| ids.contains(p)));

    let mut rows = Vec::with_capacity(top.len() + children.len());
    for task in top {
        let id = task.id.clone();
        rows.push(Row { task, depth: 0 });
        rows.extend(
            children
                .iter()
                .filter(|c| c.parent.as_deref() == Some(id.as_str()))
                .map(|c| Row { task: c.clone(), depth: 1 }),
        );
    }
    rows
}

/// Owns the API client so network calls never block the UI.
fn worker(mut client: Client, cmds: Receiver<Cmd>, out: Sender<Msg>) {
    for cmd in cmds {
        match cmd {
            Cmd::Create { list_id, title } => {
                let res = client.create(&list_id, &title).map_err(|e| e.to_string());
                if out.send(Msg::Created(list_id, res)).is_err() {
                    return;
                }
            }
            Cmd::Delete { list_id, task_id } => {
                let res = client.delete(&list_id, &task_id).map_err(|e| e.to_string());
                if out.send(Msg::Deleted(res)).is_err() {
                    return;
                }
            }
            Cmd::Patch { list_id, task_id, fields } => {
                let res = client.patch(&list_id, &task_id, &fields).map_err(|e| e.to_string());
                if out.send(Msg::Updated(list_id, res)).is_err() {
                    return;
                }
            }
            Cmd::Move { from, to, ids } => {
                let res = (|| {
                    let root = ids.first().ok_or_else(|| "no task".to_string())?;
                    for (i, id) in ids.iter().enumerate() {
                        let parent = (i > 0).then_some(root.as_str());
                        client
                            .relocate(&from, id, &to, parent)
                            .map_err(|e| e.to_string())?;
                    }
                    Ok(())
                })();
                if out.send(Msg::Moved(res)).is_err() {
                    return;
                }
            }
            Cmd::Refresh => {
                let lists = client.tasklists().map_err(|e| e.to_string());
                let ids: Vec<String> = lists.iter().flatten().map(|l| l.id.clone()).collect();
                if out.send(Msg::Lists(lists)).is_err() {
                    return;
                }
                for id in ids {
                    let tasks = client.tasks(&id).map_err(|e| e.to_string());
                    if out.send(Msg::Tasks(id, tasks)).is_err() {
                        return;
                    }
                }
            }
        }
    }
}

fn run(mut term: DefaultTerminal, mut app: App, msgs: Receiver<Msg>) -> std::io::Result<()> {
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
        while let Ok(msg) = msgs.try_recv() {
            app.on_msg(msg);
        }
        if last_poll.elapsed() >= THEME_POLL {
            app.check_theme();
            last_poll = Instant::now();
        }
    }
    Ok(())
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
    // Reuse the stored token so nothing blocks the first frame; only a first
    // run (or a wiped token) needs the browser, which wants the terminal.
    let auth = match auth::Auth::cached() {
        Some(a) => a,
        None => auth::Auth::new()?,
    };
    let client = Client::new(auth);
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (msg_tx, msg_rx) = mpsc::channel();
    thread::spawn(move || worker(client, cmd_rx, msg_tx));

    let term = ratatui::init();
    let detailed_keys = enable_key_detail();
    let res = run(term, App::new(cmd_tx, cache::load()), msg_rx);
    if detailed_keys {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    ratatui::restore();
    Ok(res?)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            list_id: "l".into(),
            task_id: "t".into(),
            field: FieldId::Title,
            cursor: buf.len(),
            buf: buf.into(),
        }
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
    }

    #[test]
    fn view_scrolls_to_keep_the_cursor_visible() {
        let mut e = edit("abcdefgh");
        assert_eq!(e.view(4), ("fgh", 3));
        e.cursor = 0;
        assert_eq!(e.view(4), ("abcd", 0));
        assert_eq!(edit("").view(4), ("", 0));
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
    }

    #[test]
    fn other_lists_skip_the_current_one() {
        let lists = [
            TaskList { id: "a".into(), title: "Inbox".into() },
            TaskList { id: "b".into(), title: "Work".into() },
            TaskList { id: "c".into(), title: "Home".into() },
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
    fn help_keys_lists_the_board_commands() {
        let keys: Vec<&str> = help_keys().iter().map(|(k, _)| *k).collect();
        for need in ["h / l", "j / k", "Enter", "Space", "c", "i", "d", "b", "m", "?", "q"] {
            assert!(keys.iter().any(|k| k.contains(need)), "missing {need} in {keys:?}");
        }
        assert!(!help_keys().is_empty());
        assert!(help_keys().iter().all(|(k, d)| !k.is_empty() && !d.is_empty()));
    }
}
