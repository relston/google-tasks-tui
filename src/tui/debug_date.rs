//! Scratch TUI for iterating on due-date editing. No Google, no board —
//! just a centered panel. Launch with `--debug-date-time`.

use std::io;

use chrono::NaiveDate;
use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyEventKind},
    layout::Rect,
    widgets::{Block, BorderType, Clear, Paragraph},
    DefaultTerminal, Frame,
};

use crate::config::Config;
use crate::theme;

const TICK: std::time::Duration = std::time::Duration::from_millis(100);
const PANEL_WIDTH: u16 = 24;
const PANEL_HEIGHT: u16 = 5;

/// `YYYY MM DD` for the panel body.
pub(super) fn format_date(date: NaiveDate) -> String {
    date.format("%Y %m %d").to_string()
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

pub(super) fn draw(frame: &mut Frame, date: NaiveDate) {
    let area = frame.area();
    let theme = theme::load(&Config::default().theme);
    let panel = panel_rect(area);
    let text = format_date(date);
    frame.render_widget(Clear, panel);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" date ")
        .border_style(ratatui::style::Style::default().fg(theme.muted))
        .style(ratatui::style::Style::default().fg(theme.fg).bg(theme.bg));
    let inner = block.inner(panel);
    frame.render_widget(block, panel);
    frame.render_widget(
        Paragraph::new(text).alignment(ratatui::layout::Alignment::Center),
        inner,
    );
}

pub(super) fn run(term: &mut DefaultTerminal, today: NaiveDate) -> io::Result<()> {
    loop {
        term.draw(|frame| draw(frame, today))?;
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
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn format_date_is_yyyy_mm_dd_with_spaces() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        assert_eq!(format_date(date), "2026 11 02");
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
    fn draw_puts_the_formatted_date_on_the_screen() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 9).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        terminal.draw(|frame| draw(frame, date)).unwrap();
        let screen = format!("{:?}", terminal.backend().buffer());
        assert!(screen.contains("2026 03 09"), "{screen}");
        assert!(screen.contains("date"), "{screen}");
    }
}
