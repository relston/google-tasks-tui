//! Modal overlays: one bounded, cleared surface for every picker.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, HighlightSpacing, List, ListItem, ListState},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

use super::super::app::App;
use super::super::picker::PickerStyle;
use super::text::{ellipsize, wrap};
use super::ViewState;

impl App {
    /// Every modal list draws through here: Browse, Move and Keyboard differ
    /// only in their rows, so layout, filtering and the empty state are shared.
    pub(super) fn draw_picker(&self, f: &mut Frame, area: Rect, t: &Theme, view: &mut ViewState) {
        let bg = self.bg(t.bg);
        let wrap_titles = self.config.wrap_titles;
        let title = self.interaction.picker_title();
        let Some(picker) = self.interaction.picker() else {
            view.picker = None;
            return;
        };

        let max_width = if picker.style() == PickerStyle::Reference {
            56
        } else {
            80
        };
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
                    if picker.style() == PickerStyle::Reference {
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
    fn modal_rect_never_exceeds_the_terminal() {
        let area = Rect::new(3, 4, 12, 2);
        let modal = modal_rect(area, 80, 30);
        assert_eq!(modal, area);
    }
}
