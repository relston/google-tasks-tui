use chrono::{DateTime, Datelike, Local, NaiveDate};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, HighlightSpacing, List, ListItem, Padding, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::api::Task;
use crate::board::Row;
use crate::keybindings::Action;
use crate::theme::Theme;

use super::app::{App, Edit, EditTarget, FieldId, Interaction, LoadState, Picker, SyncState};
use super::text::{clean, cursor_rc, ellipsize, pad_to_width, title_of, wrap};

const COL_MIN: u16 = 30;
const COL_MAX: u16 = 56;
const SHELF_W: u16 = 52;

impl App {
    fn bg(&self, color: Color) -> Color {
        if self.transparent {
            Color::Reset
        } else {
            color
        }
    }

    fn hints(&self, commands: &[(Action, &str)]) -> String {
        let hints = commands
            .iter()
            .filter_map(|(action, description)| {
                let labels = self.keymap.labels_for(*action);
                (!labels.is_empty()).then(|| format!("{} {description}", labels.join("/")))
            })
            .collect::<Vec<_>>();
        if hints.is_empty() {
            " no commands bound".into()
        } else {
            format!(" {}", hints.join(" · "))
        }
    }

    pub(super) fn draw(&mut self, f: &mut Frame) {
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
        self.draw_picker(f, body, &t);

        let hints = if self.quitting {
            let quit = self.keymap.labels_for(Action::Quit).join("/");
            format!(" waiting for pending saves… {quit} again to force quit")
        } else if self.interaction.picker().is_some_and(Picker::typing) {
            self.hints(&[
                (Action::FilterNext, "next"),
                (Action::FilterPrevious, "previous"),
                (Action::FilterAccept, "pick"),
                (Action::FilterAcceptAll, "all"),
                (Action::StopFilter, "stop filtering"),
            ])
        } else if matches!(self.interaction, Interaction::Browse(_)) {
            self.hints(&[
                (Action::PickerNext, "next"),
                (Action::PickerPrevious, "previous"),
                (Action::PickerAccept, "open"),
                (Action::PickerAcceptAll, "all"),
                (Action::StartFilter, "filter"),
                (Action::ClosePicker, "close"),
            ])
        } else if matches!(self.interaction, Interaction::Move(_)) {
            self.hints(&[
                (Action::PickerNext, "next"),
                (Action::PickerPrevious, "previous"),
                (Action::PickerAccept, "move"),
                (Action::StartFilter, "filter"),
                (Action::ClosePicker, "close"),
            ])
        } else if matches!(self.interaction, Interaction::Parent(_)) {
            self.hints(&[
                (Action::PickerNext, "next"),
                (Action::PickerPrevious, "previous"),
                (Action::PickerAccept, "choose parent"),
                (Action::StartFilter, "filter"),
                (Action::ClosePicker, "close"),
            ])
        } else if matches!(self.interaction, Interaction::Help(_)) {
            self.hints(&[
                (Action::StartFilter, "filter"),
                (Action::ClosePicker, "close"),
            ])
        } else if self.is_creating() {
            self.hints(&[(Action::SaveEdit, "create"), (Action::CancelEdit, "cancel")])
        } else if self.edit().is_some_and(|e| e.field() == FieldId::Notes) {
            self.hints(&[
                (Action::SaveEdit, "save"),
                (Action::InsertNewline, "new line"),
                (Action::CancelEdit, "cancel"),
                (Action::DeleteWord, "delete word"),
            ])
        } else if self.edit().is_some() {
            self.hints(&[
                (Action::SaveEdit, "save"),
                (Action::CancelEdit, "cancel"),
                (Action::DeleteWord, "delete word"),
                (Action::ClearToLineStart, "clear"),
            ])
        } else if self.sync != SyncState::Ready {
            " syncing with Google… board is read-only until it lands".into()
        } else if self.shelf {
            self.hints(&[
                (Action::NextField, "field"),
                (Action::Edit, "edit"),
                (Action::CreateTask, "new"),
                (Action::CreateSubtask, "subtask"),
                (Action::ChooseParent, "parent"),
                (Action::ToggleDone, "done"),
                (Action::CopyTask, "copy"),
                (Action::BrowseLinks, "link"),
                (Action::MoveTask, "move"),
                (Action::DeleteTask, "delete"),
                (Action::Help, "help"),
                (Action::Quit, "quit"),
            ])
        } else {
            self.hints(&[
                (Action::PreviousList, "previous list"),
                (Action::NextList, "next list"),
                (Action::PreviousTask, "previous task"),
                (Action::NextTask, "next task"),
                (Action::ChooseParent, "parent"),
                (Action::ToggleDone, "done"),
                (Action::ToggleDetails, "details"),
                (Action::CreateTask, "new"),
                (Action::CreateSubtask, "subtask"),
                (Action::Edit, "edit"),
                (Action::CopyTask, "copy"),
                (Action::BrowseLinks, "link"),
                (Action::MoveTask, "move"),
                (Action::DeleteTask, "delete"),
                (Action::Help, "help"),
                (Action::Quit, "quit"),
            ])
        };
        let pending = match &self.interaction {
            Interaction::ConfirmDelete(pending) => Some(pending),
            _ => None,
        };
        let status_line = match (pending, &self.error, &self.notice) {
            (Some(p), _, _) => Line::styled(
                format!(" {}", p.prompt),
                Style::default().fg(t.yellow).bold(),
            ),
            (None, Some(e), _) => Line::styled(format!(" {e}"), Style::default().fg(t.red)),
            (None, None, Some(n)) => Line::styled(format!(" {n}"), Style::default().fg(t.green)),
            (None, None, None) => Line::styled(hints, Style::default().fg(t.muted)),
        };
        f.render_widget(status_line, status);
    }

    fn draw_board(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        if self.columns.is_empty() {
            let msg = if self.loading() {
                " Loading task lists…"
            } else {
                " No task lists"
            };
            f.render_widget(
                Paragraph::new(msg).style(Style::default().fg(t.muted)),
                area,
            );
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
        let shelf_w = if self.shelf {
            SHELF_W.min(area.width / 2)
        } else {
            0
        };
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
            f.render_widget(
                Paragraph::new(msg)
                    .style(Style::default().fg(t.muted))
                    .block(block),
                area,
            );
            return;
        }

        let items: Vec<ListItem> = col
            .rows
            .iter()
            .map(|r| task_item(r, t, today, wrap_to))
            .collect();
        let highlight = if focused {
            t.selection().add_modifier(Modifier::BOLD)
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
        let heading = if parent_ref.is_some() {
            " New subtask "
        } else {
            " New task "
        };
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
        let input = Line::styled(pad_to_width(&text, w), t.selection().bold());
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
            .title(Line::styled(
                " Details ",
                Style::default().fg(t.accent).bold(),
            ))
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
        let input = t.selection();
        let mut cursor = None;

        let mut meta = Vec::new();
        let (status, color) = if task.is_done() {
            ("Completed", t.green)
        } else {
            ("Open", t.blue)
        };
        meta.push(field(
            "Status",
            Span::styled(status, Style::default().fg(color)),
            t,
        ));
        if let Some(d) = task.due_date() {
            let label = format!("{} · {}", d.format("%a %b %-d, %Y"), relative(d, today));
            let color = if task.is_done() {
                t.fg
            } else {
                due_color(d, today, t)
            };
            meta.push(field(
                "Due",
                Span::styled(label, Style::default().fg(color)),
                t,
            ));
        }
        if let Some(c) = &task.completed {
            meta.push(field("Completed", fmt_time(c).into(), t));
        }
        if let Some(p) = task
            .parent
            .as_deref()
            .and_then(|p| col.rows.iter().find(|r| r.task.id == p))
        {
            meta.push(field("Parent", title_of(&p.task).to_string().into(), t));
        }
        let subtasks: Vec<&Task> = col
            .rows
            .iter()
            .map(|r| &r.task)
            .filter(|c| c.parent.as_deref() == Some(task.id.as_str()))
            .collect();
        if !subtasks.is_empty() {
            let done = subtasks.iter().filter(|s| s.is_done()).count();
            meta.push(field(
                "Subtasks",
                format!("{done}/{} done", subtasks.len()).into(),
                t,
            ));
        }
        if task.hidden {
            meta.push(field("Hidden", "yes".into(), t));
        }
        for l in &task.links {
            let text = if l.description.is_empty() {
                &l.link
            } else {
                &l.description
            };
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
                vec![Line::styled(pad_to_width(&clean(text), w), input.bold())]
            }
            None => wrap(&title, w)
                .into_iter()
                .take(title_h as usize)
                .map(|(s, e)| {
                    Line::styled(title[s..e].to_string(), Style::default().fg(t.fg).bold())
                })
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
                cursor = Some((
                    notes_a.x + 2 + col as u16,
                    notes_a.y + (row - scroll) as u16,
                ));
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
            Paragraph::new(marked(
                vec![field("Updated", fmt_time(&task.updated).into(), t)],
                false,
                t,
            )),
            updated_a,
        );
        if let Some(pos) = cursor {
            f.set_cursor_position(pos);
        }
    }

    /// Every modal list draws through here: Browse, Move and Keyboard differ
    /// only in their rows, so layout, filtering and the empty state are shared.
    fn draw_picker(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        let bg = self.bg(t.bg);
        let wrap_titles = self.config.wrap_titles;
        let title = self.interaction.picker_title();
        let Some(picker) = self.interaction.picker_mut() else {
            return;
        };

        let max_width = if picker.is_columns() { 56 } else { 80 };
        // Two borders and the selection marker eat into the usable width.
        let text_w = (area.width.min(max_width) as usize)
            .saturating_sub(4)
            .max(1);
        let key_w = picker
            .rows()
            .map(|row| UnicodeWidthStr::width(row.title.as_str()))
            .max()
            .unwrap_or(0);

        let items: Vec<ListItem> = if picker.is_empty() {
            vec![ListItem::new(Line::styled(
                " no matches",
                Style::default().fg(t.muted),
            ))]
        } else {
            picker
                .rows()
                .map(|row| {
                    if picker.is_columns() {
                        return ListItem::new(Line::from(vec![
                            Span::styled(
                                format!(" {:<key_w$}  ", row.title),
                                Style::default().fg(t.accent).bold(),
                            ),
                            Span::styled(
                                row.subtitle.clone().unwrap_or_default(),
                                Style::default().fg(t.fg),
                            ),
                        ]));
                    }
                    let style = match row.subtitle {
                        // A bare URL is the label itself, so it keeps the link color.
                        None => Style::default().fg(t.blue),
                        Some(_) => Style::default().fg(t.fg).bold(),
                    };
                    let mut lines: Vec<Line> = if wrap_titles {
                        wrap(&row.title, text_w)
                            .into_iter()
                            .map(|(from, to)| Line::styled(row.title[from..to].to_string(), style))
                            .collect()
                    } else {
                        vec![Line::styled(ellipsize(&row.title, text_w), style)]
                    };
                    if let Some(subtitle) = &row.subtitle {
                        // The subtitle stays on one line: wrapping a URL only
                        // makes it harder to read, and it is not the label.
                        lines.push(Line::styled(
                            format!("  {}", ellipsize(subtitle, text_w.saturating_sub(2))),
                            Style::default().fg(t.muted),
                        ));
                    }
                    ListItem::new(lines)
                })
                .collect()
        };

        let rows: u16 = items.iter().map(|item| item.height() as u16).sum();
        let rect = modal_rect(area, max_width, rows.saturating_add(2));
        let filter = match (picker.typing(), picker.query().is_empty()) {
            (true, _) => format!(" / {}▏", picker.query()),
            (false, false) => format!(" / {} ", picker.query()),
            (false, true) => " / filter ".to_string(),
        };
        let filter_style = match picker.typing() || !picker.query().is_empty() {
            true => Style::default().fg(t.yellow),
            false => Style::default().fg(t.muted),
        };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(t.accent))
            .title(Line::styled(title, Style::default().fg(t.accent).bold()))
            .title_bottom(Line::styled(filter, filter_style))
            .style(Style::default().bg(bg));
        let list = List::new(items)
            .block(block)
            .highlight_style(t.selection().add_modifier(Modifier::BOLD))
            .highlight_symbol("▌")
            .highlight_spacing(HighlightSpacing::Always);
        f.render_widget(Clear, rect); // a Block restyles cells but leaves their text
        f.render_stateful_widget(list, rect, &mut picker.state);
    }
}

pub(super) fn modal_rect(area: Rect, max_width: u16, desired_height: u16) -> Rect {
    let width = area.width.min(max_width);
    let height = area.height.min(desired_height.max(3));
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

/// Prefix each line with the focus marker so focused fields stand out.
fn marked<'a>(lines: Vec<Line<'a>>, focused: bool, t: &Theme) -> Vec<Line<'a>> {
    lines
        .into_iter()
        .map(|l| {
            let mark = Span::styled(
                if focused { "▌ " } else { "  " },
                Style::default().fg(t.accent),
            );
            Line::from([vec![mark], l.spans].concat())
        })
        .collect()
}

fn field<'a>(key: &str, value: Span<'a>, t: &Theme) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{key:<11}"), Style::default().fg(t.muted)),
        value,
    ])
}

/// One row of a column. `wrap_to` is the usable text width when titles wrap,
/// `None` to keep each task on a single clipped line.
pub(super) fn task_item<'a>(
    r: &'a Row,
    t: &Theme,
    today: NaiveDate,
    wrap_to: Option<usize>,
) -> ListItem<'a> {
    let task = &r.task;
    let indent = "  ".repeat(r.depth as usize);
    let done = task.is_done();
    let (mark, mark_fg, style) = if done {
        (
            "✓ ",
            t.green,
            Style::default()
                .fg(t.muted)
                .add_modifier(Modifier::CROSSED_OUT),
        )
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
            Span::styled(
                relative(d, today),
                Style::default().fg(due_color(d, today, t)),
            ),
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
        .map(|d| {
            d.with_timezone(&Local)
                .format("%a %b %-d %Y, %-I:%M %p")
                .to_string()
        })
        .unwrap_or_else(|_| s.to_string())
}

#[cfg(test)]
mod render_tests {
    use crate::api::{Task, TaskList};
    use crate::cache;
    use crate::config::Config;
    use crate::keybindings::BindingConfig;
    use crate::tui::app::{App, Interaction, ParentPick, Picker, PickerRow, SyncState};
    use ratatui::{
        backend::TestBackend,
        crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
        Terminal,
    };
    use std::sync::mpsc;

    fn render_app(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..h)
            .flat_map(|y| {
                (0..w)
                    .map(move |x| buffer[(x, y)].symbol())
                    .chain(std::iter::once("\n"))
            })
            .collect()
    }

    fn one_list_app() -> App {
        let (tx, _rx) = mpsc::channel();
        App::new(
            tx,
            cache::Cache {
                lists: vec![cache::CachedList {
                    list: TaskList {
                        id: "list".into(),
                        title: "Underlying board".into(),
                        ..Default::default()
                    },
                    tasks: vec![Task {
                        id: "task".into(),
                        title: "Underlying task".into(),
                        status: "needsAction".into(),
                        ..Default::default()
                    }],
                }],
            },
            Config::default(),
        )
    }

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
        let mut app = App::new(
            tx,
            cached,
            Config {
                wrap_titles,
                ..Config::default()
            },
        );
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

    #[test]
    fn status_hints_use_the_active_keymap() {
        let mut app = one_list_app();
        let mut config = Config::default();
        config.keys.create_task = Some(BindingConfig::one("n"));
        let (keymap, diagnostics) = config.keymap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        app.config = config;
        app.keymap = keymap;
        app.sync = SyncState::Ready;

        let screen = render_app(&mut app, 160, 8);
        assert!(screen.contains("n new"), "{screen}");
        assert!(!screen.contains("c new"), "{screen}");
    }

    /// A control character reaching a cell is not cosmetic: printing `\r`
    /// walks the terminal cursor back to column 0 and shifts the rest of the
    /// row. Clipping hid them past a column's edge; wrapping does not.
    #[test]
    fn control_characters_never_reach_the_screen() {
        let rows = screen(
            34,
            14,
            &[
                (
                    "One",
                    &[
                        "print \r and bring with you to the hospital",
                        "bell\u{7}here",
                    ],
                ),
                ("Two\r", &["fine"]),
            ],
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
            &[
                (
                    "One",
                    &["Extend irrigation to the outer plants", "Baby go bag prep"],
                ),
                ("Two", &[long, "Irrigation for box"]),
            ],
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

    #[test]
    fn every_overlay_renders_on_a_cleared_bounded_surface() {
        let mut browse = one_list_app();
        browse.interaction = Interaction::Browse(Picker::new(vec![PickerRow::new(
            "Example".into(),
            Some("https://example.test".into()),
            "https://example.test".into(),
        )]));
        let screen = render_app(&mut browse, 50, 14);
        assert!(screen.contains("Browse Link"), "{screen}");
        assert!(screen.contains("Example"), "{screen}");

        let mut movement = one_list_app();
        movement.interaction = Interaction::Move(Picker::new(vec![PickerRow::new(
            "Destination".into(),
            None,
            "destination".into(),
        )]));
        let screen = render_app(&mut movement, 50, 14);
        assert!(screen.contains("Move"), "{screen}");
        assert!(screen.contains("Destination"), "{screen}");

        let mut parent = one_list_app();
        parent.interaction = Interaction::Parent(ParentPick {
            list_id: "list".into(),
            task_id: "task".into(),
            picker: Picker::new(vec![PickerRow::new(
                "Parent task".into(),
                None,
                "parent".into(),
            )]),
        });
        let screen = render_app(&mut parent, 50, 14);
        assert!(screen.contains("Select Parent"), "{screen}");
        assert!(screen.contains("Parent task"), "{screen}");

        let mut help = one_list_app();
        help.on_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        let screen = render_app(&mut help, 50, 20);
        assert!(screen.contains("Keyboard"), "{screen}");
        assert!(screen.contains("new task"), "{screen}");
    }

    #[test]
    fn extremely_narrow_terminals_render_without_panics_or_out_of_bounds_rows() {
        for (width, height) in [(1, 1), (2, 3), (8, 4), (12, 6)] {
            let mut app = one_list_app();
            let screen = render_app(&mut app, width, height);
            assert_eq!(
                screen.lines().count(),
                usize::from(height),
                "{width}x{height}: {screen:?}"
            );
            assert!(screen
                .lines()
                .all(|line| line.chars().count() <= width.into()));
        }
    }

    #[test]
    fn editing_renders_cjk_emoji_combining_text_and_sanitizes_controls() {
        let mut app = one_list_app();
        app.sync = SyncState::Ready;
        app.start_create(false);
        render_app(&mut app, 60, 10);
        for character in "任务🙂e\u{301}\u{7}".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }

        let screen = render_app(&mut app, 60, 10);

        assert!(screen.contains('任') && screen.contains('务'), "{screen}");
        assert!(screen.contains('🙂'), "{screen}");
        assert!(screen.contains("e\u{301}"), "{screen}");
        assert!(!screen
            .chars()
            .any(|character| { character.is_ascii_control() && character != '\n' }));
    }
}
