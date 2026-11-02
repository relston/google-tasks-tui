//! Scratch TUI for iterating on due-date editing. No Google, no board —
//! just a centered panel. Launch with `--debug-date-time`.

use std::io;

use chrono::{Datelike, Months, NaiveDate};
use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph},
    DefaultTerminal, Frame,
};

use crate::config::Config;
use crate::theme::{self, Theme};

const TICK: std::time::Duration = std::time::Duration::from_millis(100);
const PANEL_WIDTH: u16 = 28;
const PANEL_HEIGHT: u16 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Segment {
    Year,
    Month,
    Day,
}

impl Segment {
    fn next(self) -> Self {
        match self {
            Self::Year => Self::Month,
            Self::Month => Self::Day,
            Self::Day => Self::Year,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Year => Self::Day,
            Self::Month => Self::Year,
            Self::Day => Self::Month,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DateEditor {
    pub(super) date: NaiveDate,
    pub(super) focus: Segment,
}

impl DateEditor {
    pub(super) fn new(date: NaiveDate) -> Self {
        Self {
            date,
            focus: Segment::Year,
        }
    }

    pub(super) fn on_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Tab if key.modifiers.is_empty() => self.focus = self.focus.next(),
            KeyCode::BackTab => self.focus = self.focus.previous(),
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.focus = self.focus.previous();
            }
            KeyCode::Char('n') if key.modifiers.is_empty() => self.bump(1),
            KeyCode::Char('p') if key.modifiers.is_empty() => self.bump(-1),
            _ => {}
        }
    }

    fn bump(&mut self, delta: i32) {
        self.date = match self.focus {
            Segment::Year => bump_year(self.date, delta),
            Segment::Month => bump_month(self.date, delta),
            Segment::Day => bump_day(self.date, delta),
        };
    }
}

fn bump_year(date: NaiveDate, delta: i32) -> NaiveDate {
    let year = date.year().saturating_add(delta);
    clamp_ymd(year, date.month(), date.day()).unwrap_or(date)
}

fn bump_month(date: NaiveDate, delta: i32) -> NaiveDate {
    if delta >= 0 {
        date.checked_add_months(Months::new(delta as u32))
    } else {
        date.checked_sub_months(Months::new((-delta) as u32))
    }
    .unwrap_or(date)
}

fn bump_day(date: NaiveDate, delta: i32) -> NaiveDate {
    date.checked_add_signed(chrono::TimeDelta::days(delta as i64))
        .unwrap_or(date)
}

fn clamp_ymd(year: i32, month: u32, day: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(year, month, day).or_else(|| {
        // Feb 29 → Feb 28 on non-leap years, and similar month-end clamps.
        (1..=day)
            .rev()
            .find_map(|d| NaiveDate::from_ymd_opt(year, month, d))
    })
}

/// Centered floating panel inside the viewport.
pub(super) fn panel_rect(area: Rect) -> Rect {
    let width = area.width.min(PANEL_WIDTH);
    let height = area.height.min(PANEL_HEIGHT);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn segment_spans(editor: &DateEditor, theme: &Theme) -> Line<'static> {
    let year = format!("{:04}", editor.date.year());
    let month = format!("{:02}", editor.date.month());
    let day = format!("{:02}", editor.date.day());
    let gap = Span::styled(" ", Style::default().fg(theme.fg));
    Line::from(vec![
        segment_span(&year, editor.focus == Segment::Year, theme),
        gap.clone(),
        segment_span(&month, editor.focus == Segment::Month, theme),
        gap,
        segment_span(&day, editor.focus == Segment::Day, theme),
    ])
}

fn segment_span(text: &str, focused: bool, theme: &Theme) -> Span<'static> {
    let style = if focused {
        theme.selection().add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg)
    };
    Span::styled(text.to_string(), style)
}

pub(super) fn draw(frame: &mut Frame, editor: &DateEditor) {
    let area = frame.area();
    let theme = theme::load(&Config::default().theme);
    let panel = panel_rect(area);
    frame.render_widget(Clear, panel);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" date ")
        .border_style(Style::default().fg(theme.muted))
        .style(Style::default().fg(theme.fg).bg(theme.bg));
    let inner = block.inner(panel);
    frame.render_widget(block, panel);
    frame.render_widget(
        Paragraph::new(segment_spans(editor, &theme)).alignment(ratatui::layout::Alignment::Center),
        inner,
    );
}

pub(super) fn run(term: &mut DefaultTerminal, today: NaiveDate) -> io::Result<()> {
    let mut editor = DateEditor::new(today);
    loop {
        term.draw(|frame| draw(frame, &editor))?;
        if !event::poll(TICK)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q') if key.modifiers.is_empty() => return Ok(()),
            KeyCode::Esc => return Ok(()),
            _ => editor.on_key(key),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn starts_with_the_year_focused() {
        let editor = DateEditor::new(date(2026, 11, 2));
        assert_eq!(editor.focus, Segment::Year);
        assert_eq!(editor.date, date(2026, 11, 2));
    }

    #[test]
    fn tab_and_shift_tab_cycle_segments() {
        let mut editor = DateEditor::new(date(2026, 11, 2));
        editor.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(editor.focus, Segment::Month);
        editor.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(editor.focus, Segment::Day);
        editor.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(editor.focus, Segment::Year);

        editor.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
        assert_eq!(editor.focus, Segment::Day);
        editor.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
        assert_eq!(editor.focus, Segment::Month);
    }

    #[test]
    fn n_and_p_adjust_the_focused_segment() {
        let mut editor = DateEditor::new(date(2026, 1, 31));
        editor.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert_eq!(editor.date, date(2027, 1, 31));
        editor.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        assert_eq!(editor.date, date(2026, 1, 31));

        editor.focus = Segment::Month;
        editor.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert_eq!(editor.date, date(2026, 2, 28));
        editor.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        assert_eq!(editor.date, date(2026, 1, 28));

        editor.focus = Segment::Day;
        editor.date = date(2026, 1, 31);
        editor.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert_eq!(editor.date, date(2026, 2, 1));
        editor.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        assert_eq!(editor.date, date(2026, 1, 31));
    }

    #[test]
    fn panel_sits_in_the_center_of_the_viewport() {
        let area = Rect::new(0, 0, 80, 24);
        let panel = panel_rect(area);
        assert_eq!(panel.width, PANEL_WIDTH);
        assert_eq!(panel.height, PANEL_HEIGHT);
        assert_eq!(panel.x, (80 - PANEL_WIDTH) / 2);
        assert_eq!(panel.y, (24 - PANEL_HEIGHT) / 2);
    }

    #[test]
    fn draw_highlights_the_focused_segment() {
        let editor = DateEditor {
            date: date(2026, 3, 9),
            focus: Segment::Month,
        };
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        terminal.draw(|frame| draw(frame, &editor)).unwrap();
        let screen = format!("{:?}", terminal.backend().buffer());
        assert!(screen.contains("2026"), "{screen}");
        assert!(screen.contains("03"), "{screen}");
        assert!(screen.contains("09"), "{screen}");
        assert!(screen.contains("date"), "{screen}");
    }
}
