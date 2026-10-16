use std::borrow::Cow;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::model::Task;

pub(in crate::tui) fn ellipsize(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    let target = width.saturating_sub(1);
    let mut output = String::new();
    for grapheme in text.graphemes(true) {
        if UnicodeWidthStr::width(output.as_str()) + UnicodeWidthStr::width(grapheme) > target {
            break;
        }
        output.push_str(grapheme);
    }
    output.push('…');
    output
}

pub(in crate::tui) fn pad_to_width(text: &str, width: usize) -> String {
    let padding = width.saturating_sub(UnicodeWidthStr::width(text));
    format!("{text}{}", " ".repeat(padding))
}

pub(in crate::tui) fn wrap(text: &str, width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let mut output = Vec::new();
    let mut base = 0;
    for line in text.split('\n') {
        let mut start = 0;
        loop {
            let rest = &line[start..];
            if UnicodeWidthStr::width(rest) <= width {
                output.push((base + start, base + line.len()));
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
            output.push((base + start, base + start + cut));
            start += cut;
        }
        base += line.len() + 1;
    }
    output
}

pub(in crate::tui) fn cursor_rc(
    segments: &[(usize, usize)],
    text: &str,
    cursor: usize,
) -> (usize, usize) {
    let mut output = (0, 0);
    for (row, (start, end)) in segments.iter().enumerate() {
        if cursor < *start {
            break;
        }
        output = (row, UnicodeWidthStr::width(&text[*start..cursor.min(*end)]));
    }
    output
}

pub(in crate::tui) fn byte_at_column(text: &str, start: usize, end: usize, column: usize) -> usize {
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

pub(in crate::tui) fn title_of(task: &Task) -> Cow<'_, str> {
    if task.title.trim().is_empty() {
        Cow::Borrowed("(untitled)")
    } else {
        clean(&task.title)
    }
}

pub(in crate::tui) fn clean(text: &str) -> Cow<'_, str> {
    if !text
        .bytes()
        .any(|byte| byte.is_ascii_control() && byte != b'\n')
    {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.chars()
            .map(|character| {
                if character.is_ascii_control() && character != '\n' {
                    ' '
                } else {
                    character
                }
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
