//! The board: columns sized to fit, scrolled to keep focus in view, and
//! one list item per task.

use chrono::{Local, NaiveDate};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, HighlightSpacing, List, ListItem, Paragraph},
    Frame,
};

use crate::board::Row;
use crate::theme::Theme;

use super::super::app::{App, Interaction, Shelf};
use super::super::editor::{Edit, EditTarget};
use super::super::sync::LoadState;
use super::shelf::{due_color, relative};
use super::text::{clean, title_of, wrap};

pub(super) const COL_MIN: u16 = 30;
const COL_MAX: u16 = 56;
const SHELF_W: u16 = 52;

impl App {
    pub(super) fn draw_board(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
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
            self.shelf = Shelf::Closed;
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

        if self.shelf.is_open() && area.width < COL_MIN + 24 {
            self.draw_shelf(f, area, t);
            return;
        }

        // Columns share the width evenly within limits; the shelf takes a fixed slot.
        let shelf_w = if self.shelf.is_open() {
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
            if self.shelf.is_open() && i == self.focus && x < right {
                let w = shelf_w.min(right - x);
                self.draw_shelf(f, Rect::new(x, area.y, w, area.height), t);
                x += w;
            }
        }
    }

    pub(super) fn draw_column(&mut self, f: &mut Frame, area: Rect, i: usize, t: &Theme) {
        let focused = i == self.focus;
        let surface = self.bg(t.surface);
        let today = Local::now().date_naive();
        // Two borders and the selection marker sit outside the item's text.
        let wrap_to = self
            .config
            .wrap_titles
            .then(|| (area.width as usize).saturating_sub(3));
        let hide_completed = !self.show_completed;
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
            } else if hide_completed {
                "No open tasks"
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
    if let Some(d) = task.due.filter(|_| !done) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Task;

    #[test]
    fn task_titles_wrap_only_when_asked() {
        let t = Theme::default();
        let today = Local::now().date_naive();
        let long = Task {
            title: "Extend irrigation to the raised beds before the frost".into(),
            ..Task {
                id: "a".into(),
                position: "1".into(),
                ..Default::default()
            }
        };
        let rows = crate::board::ordered(vec![
            long.clone(),
            Task {
                parent: Some("a".into()),
                ..long
            },
        ]);
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
}
