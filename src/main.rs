mod api;
mod auth;
mod theme;

use std::{
    collections::HashSet,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use chrono::{DateTime, Datelike, Local, NaiveDate};
use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{
        Block, BorderType, HighlightSpacing, List, ListItem, ListState, Padding, Paragraph, Wrap,
    },
    DefaultTerminal, Frame,
};

use serde_json::{json, Value};

use api::{Client, Task, TaskList};
use theme::Theme;

const THEME_POLL: Duration = Duration::from_millis(500);
const TICK: Duration = Duration::from_millis(100);
const COL_MIN: u16 = 30;
const COL_MAX: u16 = 56;
const SHELF_W: u16 = 52;

/// Requests to the network thread.
enum Cmd {
    Refresh,
    Patch { list_id: String, task_id: String, fields: Value },
}

/// Results from the network thread.
enum Msg {
    Lists(Result<Vec<TaskList>, String>),
    Tasks(String, Result<Vec<Task>, String>),
    Updated(String, Result<Task, String>),
}

/// An in-progress title edit. `cursor` is a byte offset into `buf`.
struct Edit {
    list_id: String,
    task_id: String,
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
        self.buf.replace_range(..self.cursor, "");
        self.cursor = 0;
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
    edit: Option<Edit>,
    fetching: bool,
    /// Edits sent to Google that haven't come back yet.
    saving: usize,
    error: Option<String>,
    cmds: Sender<Cmd>,
    quit: bool,
}

impl App {
    fn new(cmds: Sender<Cmd>) -> Self {
        let mut app = Self {
            theme: theme::load(),
            fingerprint: theme::fingerprint(),
            transparent: false,
            columns: Vec::new(),
            focus: 0,
            scroll: 0,
            shelf: false,
            edit: None,
            fetching: false,
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
        let _ = self.cmds.send(Cmd::Refresh);
    }

    fn loading(&self) -> bool {
        self.fetching || self.columns.iter().any(|c| !c.loaded)
    }

    fn start_edit(&mut self) {
        let Some(col) = self.columns.get(self.focus) else { return };
        let Some(row) = col.selected() else { return };
        self.edit = Some(Edit {
            list_id: col.list.id.clone(),
            task_id: row.task.id.clone(),
            cursor: row.task.title.len(),
            buf: row.task.title.clone(),
        });
        self.shelf = true;
    }

    fn commit_edit(&mut self) {
        let Some(e) = self.edit.take() else { return };
        let title = e.buf.trim().to_string();
        if title.is_empty() {
            self.error = Some("A task needs a title".into());
            self.edit = Some(e);
            return;
        }
        // Show the new title straight away; Google confirms it a moment later.
        if let Some(row) = self
            .columns
            .iter_mut()
            .find(|c| c.list.id == e.list_id)
            .and_then(|c| c.rows.iter_mut().find(|r| r.task.id == e.task_id))
        {
            row.task.title = title.clone();
        }
        self.error = None;
        self.send_patch(e.list_id, e.task_id, json!({ "title": title }));
    }

    /// Tick or untick the selected task.
    fn toggle_done(&mut self) {
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

    fn on_key(&mut self, k: KeyEvent) {
        if self.edit.is_some() {
            self.edit_key(k);
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
            KeyCode::Enter => self.shelf = !self.shelf && self.selected_task().is_some(),
            KeyCode::Char('i') => self.start_edit(),
            KeyCode::Char(' ') => self.toggle_done(),
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('t') => self.transparent = !self.transparent,
            _ => {}
        }
    }

    /// Keys while editing a title. Enter saves, Esc discards.
    fn edit_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Esc => {
                self.edit = None;
                return;
            }
            KeyCode::Enter => {
                self.commit_edit();
                return;
            }
            _ => {}
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let Some(e) = self.edit.as_mut() else { return };
        match k.code {
            KeyCode::Char('w') if ctrl => e.kill_word(),
            KeyCode::Char('u') if ctrl => e.kill_to_start(),
            KeyCode::Char('a') if ctrl => e.cursor = 0,
            KeyCode::Char('e') if ctrl => e.cursor = e.buf.len(),
            KeyCode::Char(c) if !ctrl => e.insert(c),
            KeyCode::Backspace => e.backspace(),
            KeyCode::Delete => e.delete(),
            KeyCode::Left => e.left(),
            KeyCode::Right => e.right(),
            KeyCode::Home => e.cursor = 0,
            KeyCode::End => e.cursor = e.buf.len(),
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

        let hints = if self.edit.is_some() {
            " ⏎ save · Esc cancel · ^W delete word · ^U clear"
        } else {
            " h/l list · j/k task · ␣ done · ⏎ details · i edit · r refresh · t transparent · q quit"
        };
        let status_line = match &self.error {
            Some(e) => Line::styled(format!(" {e}"), Style::default().fg(t.red)),
            None => Line::styled(hints, Style::default().fg(t.muted)),
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
        if self.selected_task().is_none() {
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

    fn draw_shelf(&self, f: &mut Frame, area: Rect, t: &Theme) {
        let col = &self.columns[self.focus];
        let Some(row) = col.selected() else { return };
        let task = &row.task;
        let today = Local::now().date_naive();
        let muted = Style::default().fg(t.muted);

        // Inside the border and the block's one-column padding.
        let inner_w = area.width.saturating_sub(4) as usize;
        let editing = self
            .edit
            .as_ref()
            .filter(|e| e.task_id == task.id && e.list_id == col.list.id);
        let mut cursor = None;
        let title_line = match editing {
            Some(e) => {
                let (text, offset) = e.view(inner_w);
                cursor = Some((area.x + 2 + offset as u16, area.y + 1));
                Line::styled(
                    format!("{text:<inner_w$}"),
                    Style::default().fg(t.sel_fg).bg(t.sel_bg).bold(),
                )
            }
            None => Line::styled(title_of(task).to_string(), Style::default().fg(t.fg).bold()),
        };
        let mut lines = vec![title_line, Line::default()];
        let (status, color) = if task.is_done() { ("Completed", t.green) } else { ("Open", t.blue) };
        lines.push(field("Status", Span::styled(status, Style::default().fg(color)), t));
        if let Some(d) = task.due_date() {
            let label = format!("{} · {}", d.format("%a %b %-d, %Y"), relative(d, today));
            let color = if task.is_done() { t.fg } else { due_color(d, today, t) };
            lines.push(field("Due", Span::styled(label, Style::default().fg(color)), t));
        }
        if let Some(c) = &task.completed {
            lines.push(field("Completed", fmt_time(c).into(), t));
        }
        if let Some(p) = task.parent.as_deref().and_then(|p| col.rows.iter().find(|r| r.task.id == p)) {
            lines.push(field("Parent", title_of(&p.task).to_string().into(), t));
        }
        let subtasks: Vec<&Task> =
            col.rows.iter().map(|r| &r.task).filter(|c| c.parent.as_deref() == Some(task.id.as_str())).collect();
        if !subtasks.is_empty() {
            let done = subtasks.iter().filter(|s| s.is_done()).count();
            lines.push(field("Subtasks", format!("{done}/{} done", subtasks.len()).into(), t));
        }
        if task.hidden {
            lines.push(field("Hidden", "yes".into(), t));
        }

        if let Some(notes) = task.notes.as_deref().filter(|n| !n.trim().is_empty()) {
            lines.push(Line::default());
            lines.push(Line::styled("Notes", muted.bold()));
            lines.extend(notes.lines().map(|l| Line::raw(l.to_string())));
        }
        if !task.links.is_empty() {
            lines.push(Line::default());
            lines.push(Line::styled("Links", muted.bold()));
            for l in &task.links {
                if !l.description.is_empty() {
                    lines.push(Line::raw(l.description.clone()));
                }
                lines.push(Line::styled(l.link.clone(), Style::default().fg(t.blue)));
            }
        }
        lines.push(Line::default());
        lines.push(field("Updated", fmt_time(&task.updated).into(), t));

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(t.accent))
            .title(Line::styled(" Details ", Style::default().fg(t.accent).bold()))
            .padding(Padding::horizontal(1))
            .style(Style::default().bg(self.bg(t.bg)));
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), area);
        if let Some(pos) = cursor {
            f.set_cursor_position(pos);
        }
    }
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
            Cmd::Patch { list_id, task_id, fields } => {
                let res = client.patch(&list_id, &task_id, &fields).map_err(|e| e.to_string());
                if out.send(Msg::Updated(list_id, res)).is_err() {
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

fn main() -> auth::Result<()> {
    // Auth first: it may need the terminal for the browser sign-in prompt.
    let client = Client::new(auth::Auth::new()?);
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (msg_tx, msg_rx) = mpsc::channel();
    thread::spawn(move || worker(client, cmd_rx, msg_tx));

    let term = ratatui::init();
    let res = run(term, App::new(cmd_tx), msg_rx);
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
}
