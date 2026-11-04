//! Floating list-title editor. Shift+i renames; Shift+t creates.

use ratatui::{
    layout::Rect,
    style::Style,
    widgets::{Block, BorderType, Clear, Paragraph},
    Frame,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

use super::view::text::clean;

const PANEL_WIDTH: u16 = 40;
const PANEL_HEIGHT: u16 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ListEditKind {
    Rename,
    Create,
}

/// In-progress create or rename of a task list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ListEdit {
    pub(super) kind: ListEditKind,
    pub(super) list_id: String,
    pub(super) etag: String,
    pub(super) buf: String,
    pub(super) cursor: usize,
}

impl ListEdit {
    pub(super) fn rename(list_id: String, etag: String, title: String) -> Self {
        let cursor = title.len();
        Self {
            kind: ListEditKind::Rename,
            list_id,
            etag,
            buf: title,
            cursor,
        }
    }

    pub(super) fn create() -> Self {
        Self {
            kind: ListEditKind::Create,
            list_id: String::new(),
            etag: String::new(),
            buf: String::new(),
            cursor: 0,
        }
    }

    pub(super) fn is_create(&self) -> bool {
        self.kind == ListEditKind::Create
    }

    pub(super) fn panel_title(&self) -> &'static str {
        match self.kind {
            ListEditKind::Rename => " rename list ",
            ListEditKind::Create => " new list ",
        }
    }

    pub(super) fn insert(&mut self, c: char) {
        if c == '\n' || c == '\r' || c.is_control() {
            return;
        }
        self.buf.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub(super) fn paste(&mut self, text: &str) {
        let text: String = text
            .chars()
            .map(|c| {
                if c == '\n' || c == '\r' || c.is_control() {
                    ' '
                } else {
                    c
                }
            })
            .collect();
        self.buf.insert_str(self.cursor, &text);
        self.cursor += text.len();
    }

    pub(super) fn left(&mut self) {
        if let Some((i, _)) = self.buf[..self.cursor].grapheme_indices(true).next_back() {
            self.cursor = i;
        }
    }

    pub(super) fn right(&mut self) {
        if let Some(grapheme) = self.buf[self.cursor..].graphemes(true).next() {
            self.cursor += grapheme.len();
        }
    }

    pub(super) fn backspace(&mut self) {
        let end = self.cursor;
        if let Some((i, _)) = self.buf[..end].grapheme_indices(true).next_back() {
            self.buf.replace_range(i..end, "");
            self.cursor = i;
        }
    }

    pub(super) fn delete(&mut self) {
        if let Some(grapheme) = self.buf[self.cursor..].graphemes(true).next() {
            let end = self.cursor + grapheme.len();
            self.buf.replace_range(self.cursor..end, "");
        }
    }
}

fn panel_rect(area: Rect) -> Rect {
    let width = area.width.min(PANEL_WIDTH);
    let height = area.height.min(PANEL_HEIGHT);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

pub(super) fn draw_panel(frame: &mut Frame, area: Rect, edit: &ListEdit, theme: &Theme) {
    let panel = panel_rect(area);
    frame.render_widget(Clear, panel);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(edit.panel_title())
        .border_style(Style::default().fg(theme.muted))
        .style(Style::default().fg(theme.fg).bg(theme.bg));
    let inner = block.inner(panel);
    frame.render_widget(block, panel);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let text = clean(&edit.buf).into_owned();
    let cursor_cells = UnicodeWidthStr::width(&edit.buf[..edit.cursor.min(edit.buf.len())]);
    frame.render_widget(Paragraph::new(text), inner);
    let cursor_x = inner
        .x
        .saturating_add(cursor_cells as u16)
        .min(inner.x + inner.width.saturating_sub(1));
    frame.set_cursor_position((cursor_x, inner.y));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_backspace_edit_the_title() {
        let mut edit = ListEdit::rename("l".into(), "e".into(), "Work".into());
        assert_eq!(edit.cursor, 4);
        edit.backspace();
        assert_eq!(edit.buf, "Wor");
        edit.insert('k');
        assert_eq!(edit.buf, "Work");
        edit.insert('s');
        assert_eq!(edit.buf, "Works");
        edit.left();
        edit.insert('!');
        assert_eq!(edit.buf, "Work!s");
        edit.backspace();
        assert_eq!(edit.buf, "Works");
        edit.delete();
        assert_eq!(edit.buf, "Work");
    }
}
