//! The details shelf: task metadata, the title and notes fields, and the
//! inline editor.

use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Padding, Paragraph},
    Frame,
};

use crate::keys::Action;
use crate::model::Task;
use crate::theme::Theme;

use super::super::app::{App, Shelf};
use super::super::editor::{Edit, EditTarget, FieldId};
use super::text::{clean, cursor_rc, pad_to_width, title_of, wrap};

impl App {
    /// The shelf while a new task is being typed: just a title input.
    pub(super) fn draw_new_shelf(&self, f: &mut Frame, area: Rect, t: &Theme) {
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
            let cancel = self.keymap.labels_for(Action::CancelEdit).join("/");
            f.render_widget(
                Paragraph::new(format!("Terminal too small; {cancel} cancels"))
                    .style(Style::default().fg(t.yellow)),
                inner,
            );
            return;
        }
        let w = inner.width as usize - 2;
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

    pub(super) fn draw_shelf(&self, f: &mut Frame, area: Rect, t: &Theme) {
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
            let close = self.keymap.labels_for(Action::CloseShelf).join("/");
            f.render_widget(
                Paragraph::new(format!("Terminal too small; {close} closes details"))
                    .style(Style::default().fg(t.yellow)),
                inner,
            );
            return;
        }
        // Two columns go to the focus marker.
        let w = inner.width as usize - 2;
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
        if let Some(d) = task.due {
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
        if let Some(parent) = task
            .parent
            .as_deref()
            .and_then(|parent| self.board.task(&col.list.id, parent))
        {
            meta.push(field("Parent", title_of(parent).to_string().into(), t));
        }
        let subtasks: Vec<&Task> = self.board.subtasks(&col.list.id, &task.id).collect();
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
        let on_title = self.shelf == Shelf::Open(FieldId::Title);
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
        let on_notes = self.shelf == Shelf::Open(FieldId::Notes);
        f.render_widget(Paragraph::new(marked(notes_lines, on_notes, t)), notes_a);
        f.render_widget(
            Paragraph::new(marked(
                vec![field(
                    "Updated",
                    task.updated
                        .as_ref()
                        .map(fmt_time)
                        .unwrap_or_default()
                        .into(),
                    t,
                )],
                false,
                t,
            )),
            updated_a,
        );
        if let Some(pos) = cursor {
            f.set_cursor_position(pos);
        }
    }
}

/// Prefix each line with the focus marker so focused fields stand out.
pub(super) fn marked<'a>(lines: Vec<Line<'a>>, focused: bool, t: &Theme) -> Vec<Line<'a>> {
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

pub(super) fn relative(d: NaiveDate, today: NaiveDate) -> String {
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

pub(super) fn due_color(d: NaiveDate, today: NaiveDate, t: &Theme) -> Color {
    match (d - today).num_days() {
        n if n < 0 => t.red,
        0 => t.yellow,
        _ => t.muted,
    }
}

fn fmt_time(time: &DateTime<Utc>) -> String {
    time.with_timezone(&Local)
        .format("%a %b %-d %Y, %-I:%M %p")
        .to_string()
}
