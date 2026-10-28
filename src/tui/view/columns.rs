//! The board: columns sized to fit, scrolled to keep focus in view, and
//! one list item per task.

use chrono::{Local, NaiveDate};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, HighlightSpacing, List, ListItem, ListState, Padding, Paragraph},
    Frame,
};

use crate::board::Row;
use crate::theme::Theme;

use super::super::app::App;
use super::super::sync::LoadState;
use super::shelf::{due_color, relative};
use super::text::{clean, title_of, wrap};
use super::ViewState;

const COL_MIN: u16 = 30;
const COL_MAX: u16 = 56;
const SHELF_W: u16 = 52;

impl App {
    /// The shelf is drawn while open with a task or a new-task draft to show.
    fn shelf_shown(&self) -> bool {
        self.shelf.is_open() && (self.selected_task().is_some() || self.is_creating())
    }

    /// Column and shelf widths: columns share the width evenly within
    /// limits, and the shelf takes a fixed slot.
    fn widths(&self, width: u16, shelf: bool) -> (u16, u16) {
        let shelf_w = if shelf { SHELF_W.min(width / 2) } else { 0 };
        let columns = self.columns.len().max(1) as u16;
        let col_w = ((width - shelf_w) / columns).clamp(COL_MIN, COL_MAX);
        (col_w, shelf_w)
    }

    /// The editor's text width in the current layout, or `None` when its
    /// shelf is too small to show it. Input uses it to move the cursor.
    pub(in crate::tui) fn editor_width(&self) -> Option<usize> {
        if self.columns.is_empty() || !self.shelf_shown() {
            return None;
        }
        let body = self.viewport.height.saturating_sub(2);
        let width = if self.viewport.width < COL_MIN + 24 {
            self.viewport.width
        } else {
            let (col_w, shelf_w) = self.widths(self.viewport.width, true);
            shelf_w.min(self.viewport.width.saturating_sub(col_w))
        };
        let inner = Block::bordered()
            .padding(Padding::horizontal(1))
            .inner(Rect::new(0, 0, width, body));
        let min_height = if self.is_creating() { 3 } else { 8 };
        (inner.width >= 8 && inner.height >= min_height).then(|| inner.width as usize - 2)
    }

    pub(super) fn draw_board(&self, f: &mut Frame, area: Rect, t: &Theme, view: &mut ViewState) {
        if self.columns.is_empty() {
            let msg = if self.loading() {
                " Loading task lists…"
            } else {
                " No task lists"
            };
            f.render_widget(
                Paragraph::new(msg).style(Style::default().fg(t.subtle)),
                area,
            );
            return;
        }
        let focus = self.focus.min(self.columns.len() - 1);
        let shelf = self.shelf_shown();
        if shelf && area.width < COL_MIN + 24 {
            self.draw_shelf(f, area, t);
            return;
        }
        let (col_w, shelf_w) = self.widths(area.width, shelf);

        // Scroll just enough to keep the focused column (and its shelf) in view.
        view.scroll = view.scroll.min(focus);
        let span = |from: usize| (focus - from + 1) as u16 * col_w + shelf_w;
        while view.scroll < focus && span(view.scroll) > area.width {
            view.scroll += 1;
        }

        let right = area.right();
        let mut x = area.x;
        for i in view.scroll..self.columns.len() {
            if x >= right {
                break;
            }
            let w = col_w.min(right - x);
            self.draw_column(f, Rect::new(x, area.y, w, area.height), i, t, view);
            x += w;
            if shelf && i == focus && x < right {
                let w = shelf_w.min(right - x);
                self.draw_shelf(f, Rect::new(x, area.y, w, area.height), t);
                x += w;
            }
        }
    }

    pub(super) fn draw_column(
        &self,
        f: &mut Frame,
        area: Rect,
        i: usize,
        t: &Theme,
        view: &mut ViewState,
    ) {
        let focused = i == self.focus;
        let surface = self.bg(t.surface);
        let today = Local::now().date_naive();
        // Two borders and the selection marker sit outside the item's text.
        let wrap_to = self
            .config
            .wrap_titles
            .then(|| (area.width as usize).saturating_sub(3));
        let hide_completed = !self.show_completed;
        let col = &self.columns[i];

        let open = col.rows.iter().filter(|r| !r.task.is_done()).count();
        let title_fg = if focused { t.fg } else { t.subtle };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(if focused { t.accent } else { t.muted }))
            .title(Line::styled(
                format!(" {} ", clean(&col.list.title)),
                Style::default().fg(title_fg).bold(),
            ))
            .title(Line::styled(format!(" {open} "), Style::default().fg(t.subtle)).right_aligned())
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
                    .style(Style::default().fg(t.subtle))
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
        let offset = view.offsets.get(&col.list.id).copied().unwrap_or(0);
        let mut state = ListState::default()
            .with_offset(offset)
            .with_selected(col.selected_index());
        f.render_stateful_widget(list, area, &mut state);
        view.offsets.insert(col.list.id.clone(), state.offset());
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
                .fg(t.subtle)
                .add_modifier(Modifier::CROSSED_OUT),
        )
    } else {
        ("○ ", t.subtle, Style::default().fg(t.fg))
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
                spans.push(Span::styled(" ≡", Style::default().fg(t.subtle)));
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
