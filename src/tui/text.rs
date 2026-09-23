use std::borrow::Cow;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::api::Task;

pub(super) fn ellipsize(text: &str, width: usize) -> String {
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

pub(super) fn pad_to_width(text: &str, width: usize) -> String {
    let padding = width.saturating_sub(UnicodeWidthStr::width(text));
    format!("{text}{}", " ".repeat(padding))
}

pub(super) fn wrap(text: &str, width: usize) -> Vec<(usize, usize)> {
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

pub(super) fn cursor_rc(segments: &[(usize, usize)], text: &str, cursor: usize) -> (usize, usize) {
    let mut output = (0, 0);
    for (row, (start, end)) in segments.iter().enumerate() {
        if cursor < *start {
            break;
        }
        output = (row, UnicodeWidthStr::width(&text[*start..cursor.min(*end)]));
    }
    output
}

pub(super) fn byte_at_column(text: &str, start: usize, end: usize, column: usize) -> usize {
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

pub(super) fn title_of(task: &Task) -> Cow<'_, str> {
    if task.title.trim().is_empty() {
        Cow::Borrowed("(untitled)")
    } else {
        clean(&task.title)
    }
}

pub(super) fn clean(text: &str) -> Cow<'_, str> {
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
