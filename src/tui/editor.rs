//! The title/notes edit buffer. `cursor` is a byte offset; movement is
//! grapheme- and terminal-cell-aware.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::view::text::{byte_at_column, cursor_rc, wrap};

/// The editable fields of the shelf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FieldId {
    Title,
    Notes,
}

impl FieldId {
    pub(super) fn other(self) -> Self {
        match self {
            Self::Title => Self::Notes,
            Self::Notes => Self::Title,
        }
    }
}

pub(super) enum EditTarget {
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
pub(super) struct Edit {
    pub(super) target: EditTarget,
    pub(super) buf: String,
    pub(super) cursor: usize,
}

impl Edit {
    pub(super) fn field(&self) -> FieldId {
        match self.target {
            EditTarget::Existing { field, .. } => field,
            EditTarget::New { .. } => FieldId::Title,
        }
    }

    pub(super) fn existing(&self) -> Option<(&str, &str, FieldId)> {
        match &self.target {
            EditTarget::Existing {
                list_id,
                task_id,
                field,
            } => Some((list_id, task_id, *field)),
            EditTarget::New { .. } => None,
        }
    }

    pub(super) fn is_new(&self) -> bool {
        matches!(self.target, EditTarget::New { .. })
    }

    pub(super) fn insert(&mut self, c: char) {
        self.buf.insert(self.cursor, c);
        self.cursor += c.len_utf8();
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
        self.left();
        self.buf.replace_range(self.cursor..end, "");
    }

    pub(super) fn delete(&mut self) {
        if let Some(grapheme) = self.buf[self.cursor..].graphemes(true).next() {
            let end = self.cursor + grapheme.len();
            self.buf.replace_range(self.cursor..end, "");
        }
    }

    pub(super) fn kill_word(&mut self) {
        let head = self.buf[..self.cursor].trim_end_matches(char::is_whitespace);
        let start = head
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map_or(0, |(i, c)| i + c.len_utf8());
        self.buf.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    pub(super) fn kill_to_start(&mut self) {
        let start = self.line_start();
        self.buf.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    pub(super) fn line_start(&self) -> usize {
        self.buf[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    pub(super) fn line_end(&self) -> usize {
        self.buf[self.cursor..]
            .find('\n')
            .map_or(self.buf.len(), |i| self.cursor + i)
    }

    pub(super) fn step_visual_line(&mut self, width: usize, down: bool) {
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
    pub(super) fn view(&self, width: usize) -> (&str, usize) {
        let width = width.max(1);
        let mut boundaries: Vec<usize> = self
            .buf
            .grapheme_indices(true)
            .map(|(index, _)| index)
            .collect();
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

#[cfg(test)]
mod tests {
    use crate::tui::test_support::*;

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
}
