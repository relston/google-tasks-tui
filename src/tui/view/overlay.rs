//! Modal overlays: one bounded, cleared surface for every picker.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

use super::super::app::{App, Interaction};
use super::super::debug_date;
use super::super::picker::{Picker, PickerStyle};
use super::text::{ellipsize, wrap};
use super::ViewState;

impl App {
    /// Floating due-date panel over a dimmed board.
    pub(super) fn draw_date_edit(&self, f: &mut Frame, area: Rect, t: &Theme) {
        let Interaction::DateEdit(edit) = &self.interaction else {
            return;
        };
        dim_backdrop(f.buffer_mut(), area);
        debug_date::draw_panel(f, area, &edit.editor, t);
    }

    /// Every modal list draws through here, over a dimmed board. Browse and
    /// Move share one list layout; the read-only Keyboard card flows into
    /// columns. Filtering and the empty state are shared.
    pub(super) fn draw_picker(&self, f: &mut Frame, area: Rect, t: &Theme, view: &mut ViewState) {
        let bg = self.bg(t.bg);
        let wrap_titles = self.config.wrap_titles;
        let title = self.interaction.picker_title();
        let Some(picker) = self.interaction.picker() else {
            view.picker = None;
            return;
        };

        dim_backdrop(f.buffer_mut(), area);
        if picker.style() == PickerStyle::Reference {
            self.draw_reference(f, area, t, picker, title);
            view.picker = Some((title, 0));
            return;
        }

        let max_width = 80;
        // Two borders and the selection marker eat into the usable width.
        let text_w = (area.width.min(max_width) as usize)
            .saturating_sub(4)
            .max(1);

        let items: Vec<ListItem> = if picker.is_empty() {
            vec![no_matches(t)]
        } else {
            picker
                .rows()
                .map(|row| {
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
                            Style::default().fg(t.subtle),
                        ));
                    }
                    ListItem::new(lines)
                })
                .collect()
        };

        let rows: u16 = items.iter().map(|item| item.height() as u16).sum();
        let rect = modal_rect(area, max_width, rows.saturating_add(2));
        let block = picker_block(title, picker, t, bg);
        let list = List::new(items)
            .block(block)
            .highlight_style(t.selection().add_modifier(Modifier::BOLD))
            .highlight_symbol("▌")
            .highlight_spacing(HighlightSpacing::Always);
        f.render_widget(Clear, rect); // a Block restyles cells but leaves their text
        let offset = match view.picker {
            Some((open, offset)) if open == title => offset,
            _ => 0,
        };
        let mut state = ListState::default()
            .with_offset(offset)
            .with_selected(picker.selected_index());
        f.render_stateful_widget(list, rect, &mut state);
        view.picker = Some((title, state.offset()));
    }
    /// The Keyboard card: key and description pairs flowed down as many
    /// columns as fit (up to three), so the card is wide and short rather
    /// than one tall list with clipped descriptions.
    fn draw_reference(
        &self,
        f: &mut Frame,
        area: Rect,
        t: &Theme,
        picker: &Picker,
        title: &'static str,
    ) {
        let entries: Vec<(&str, &str)> = picker
            .rows()
            .map(|row| {
                (
                    row.title.as_str(),
                    row.subtitle.as_deref().unwrap_or_default(),
                )
            })
            .collect();
        let columns = reference_columns(&entries, area.width as usize);
        let rows = columns.first().map_or(0, |column| column.len);

        let lines: Vec<Line> = if entries.is_empty() {
            vec![Line::styled(" no matches", Style::default().fg(t.subtle))]
        } else {
            (0..rows)
                .map(|row| {
                    let mut spans = Vec::new();
                    for column in &columns {
                        if row >= column.len {
                            break;
                        }
                        let (key, desc) = entries[column.start + row];
                        let key_w = column.key_w;
                        spans.push(Span::styled(
                            format!(" {key:<key_w$}{:REFERENCE_GAP$}", ""),
                            Style::default().fg(t.accent).bold(),
                        ));
                        let desc = ellipsize(desc, column.desc_w);
                        let pad = column.desc_w.saturating_sub(desc.width()) + REFERENCE_GUTTER;
                        spans.push(Span::styled(
                            format!("{desc}{:pad$}", ""),
                            Style::default().fg(t.fg),
                        ));
                    }
                    Line::from(spans)
                })
                .collect()
        };

        let inner: usize = columns.iter().map(ReferenceColumn::width).sum();
        let width = (inner + 2).saturating_sub(REFERENCE_GUTTER - 1) as u16;
        let rect = modal_rect(area, width.max(16), lines.len() as u16 + 2);
        let block = picker_block(title, picker, t, self.bg(t.bg));
        f.render_widget(Clear, rect);
        f.render_widget(Paragraph::new(lines).block(block), rect);
    }
}

/// Space between a key and its description in the Keyboard card.
const REFERENCE_GAP: usize = 2;
/// Space between columns of the Keyboard card.
const REFERENCE_GUTTER: usize = 3;
const REFERENCE_MAX_COLUMNS: usize = 3;

/// One column of the Keyboard card: a run of entries and the widths they need.
#[derive(Debug, PartialEq)]
struct ReferenceColumn {
    start: usize,
    len: usize,
    key_w: usize,
    desc_w: usize,
}

impl ReferenceColumn {
    /// Cells this column takes, including its leading space and gutter.
    fn width(&self) -> usize {
        1 + self.key_w + REFERENCE_GAP + self.desc_w + REFERENCE_GUTTER
    }
}

/// Flow entries down the most columns (up to three) whose natural widths fit
/// in `width`, each sized to its own entries. When even one column is too
/// wide, its descriptions are clipped to fit.
fn reference_columns(entries: &[(&str, &str)], width: usize) -> Vec<ReferenceColumn> {
    let split = |count: usize| -> Vec<ReferenceColumn> {
        let rows = entries.len().div_ceil(count).max(1);
        (0..count)
            .map(|index| index * rows)
            .take_while(|start| *start < entries.len().max(1))
            .map(|start| {
                let chunk = &entries[start.min(entries.len())..(start + rows).min(entries.len())];
                ReferenceColumn {
                    start,
                    len: chunk.len(),
                    key_w: chunk.iter().map(|(key, _)| key.width()).max().unwrap_or(0),
                    desc_w: chunk
                        .iter()
                        .map(|(_, desc)| desc.width())
                        .max()
                        .unwrap_or(0),
                }
            })
            .collect()
    };
    // Two borders, less the last column's unused gutter.
    let fits = |columns: &[ReferenceColumn]| {
        columns.iter().map(ReferenceColumn::width).sum::<usize>() + 2 - REFERENCE_GUTTER <= width
    };
    for count in (2..=REFERENCE_MAX_COLUMNS.min(entries.len())).rev() {
        let columns = split(count);
        if fits(&columns) {
            return columns;
        }
    }
    let mut columns = split(1);
    if let Some(column) = columns.first_mut() {
        let fixed = column.width() - column.desc_w + 2 - REFERENCE_GUTTER;
        column.desc_w = column.desc_w.min(width.saturating_sub(fixed)).max(1);
    }
    columns
}

fn no_matches(t: &Theme) -> ListItem<'static> {
    ListItem::new(Line::styled(" no matches", Style::default().fg(t.subtle)))
}

fn picker_block<'a>(title: &'a str, picker: &Picker, t: &Theme, bg: Color) -> Block<'a> {
    let filter = match (picker.typing(), picker.query().is_empty()) {
        (true, _) => format!(" / {}▏", picker.query()),
        (false, false) => format!(" / {} ", picker.query()),
        (false, true) => " / filter ".to_string(),
    };
    let filter_style = match picker.typing() || !picker.query().is_empty() {
        true => Style::default().fg(t.yellow),
        false => Style::default().fg(t.subtle),
    };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.accent))
        .title(Line::styled(title, Style::default().fg(t.accent).bold()))
        .title_bottom(Line::styled(filter, filter_style))
        .style(Style::default().bg(bg))
}

/// How much of each 24-bit color the board keeps behind an overlay.
const BACKDROP_KEEP: f32 = 0.6;

/// Push the board back while an overlay is open: 24-bit colors darken, and
/// anything else (ANSI colors, the terminal default, `NO_COLOR`) gets the
/// terminal's dim attribute.
pub(super) fn dim_backdrop(buf: &mut Buffer, area: Rect) {
    let darken = |color: Color| match color {
        Color::Rgb(r, g, b) => {
            let scale = |c: u8| (f32::from(c) * BACKDROP_KEEP).round() as u8;
            Some(Color::Rgb(scale(r), scale(g), scale(b)))
        }
        _ => None,
    };
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let Some(cell) = buf.cell_mut((x, y)) else {
                continue;
            };
            match darken(cell.fg) {
                Some(fg) => cell.fg = fg,
                None => cell.modifier.insert(Modifier::DIM),
            }
            if let Some(bg) = darken(cell.bg) {
                cell.bg = bg;
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keyboard_card_uses_the_most_columns_that_fit() {
        let entries = vec![("k", "short"); 9];
        let one = 1 + 1 + REFERENCE_GAP + 5 + REFERENCE_GUTTER;

        let columns = reference_columns(&entries, 3 * one + 2 - REFERENCE_GUTTER);
        assert_eq!(columns.len(), 3);
        assert_eq!(columns.iter().map(|c| c.len).collect::<Vec<_>>(), [3, 3, 3]);
        assert_eq!(columns[2].start, 6);

        assert_eq!(reference_columns(&entries, 3 * one).len(), 3);
        assert_eq!(reference_columns(&entries, 2 * one).len(), 2);
        assert_eq!(reference_columns(&entries, one).len(), 1);
    }

    #[test]
    fn each_keyboard_column_is_sized_to_its_own_keys() {
        let entries = [
            ("q", "quit"),
            ("esc", "close"),
            ("shift+enter / alt+enter", "insert newline"),
            ("x", "y"),
        ];
        let columns = reference_columns(&entries, 200);
        assert_eq!(columns.len(), 2);
        assert_eq!((columns[0].key_w, columns[1].key_w), (3, 23));
    }

    #[test]
    fn a_single_keyboard_column_clips_descriptions_to_the_terminal() {
        let entries = [("q", "a description far too long for the screen")];
        let columns = reference_columns(&entries, 20);
        assert_eq!(columns.len(), 1);
        assert_eq!(columns[0].width() + 2 - REFERENCE_GUTTER, 20);
    }

    #[test]
    fn the_backdrop_darkens_rgb_and_dims_everything_else() {
        let area = Rect::new(0, 0, 2, 1);
        let mut buf = Buffer::empty(area);
        buf[(0, 0)]
            .set_fg(Color::Rgb(200, 100, 50))
            .set_bg(Color::Rgb(10, 20, 30));
        buf[(1, 0)].set_fg(Color::Blue);

        dim_backdrop(&mut buf, area);

        assert_eq!(buf[(0, 0)].fg, Color::Rgb(120, 60, 30));
        assert_eq!(buf[(0, 0)].bg, Color::Rgb(6, 12, 18));
        assert!(!buf[(0, 0)].modifier.contains(Modifier::DIM));
        assert_eq!(buf[(1, 0)].fg, Color::Blue);
        assert!(buf[(1, 0)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn modal_rect_never_exceeds_the_terminal() {
        let area = Rect::new(3, 4, 12, 2);
        let modal = modal_rect(area, 80, 30);
        assert_eq!(modal, area);
    }
}
