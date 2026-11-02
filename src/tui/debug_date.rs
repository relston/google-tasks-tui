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
/// Matches `cal`: seven two-digit day columns with single spaces.
const CAL_WIDTH: usize = 20;
const PANEL_WIDTH: u16 = 22;
const PICKER_HEIGHT: u16 = 3;
const GAP: u16 = 1;
const PREVIEW_CHROME: u16 = 4; // borders + month title + weekday header

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

    fn width(self) -> usize {
        match self {
            Self::Year => 4,
            Self::Month | Self::Day => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DateEditor {
    pub(super) date: NaiveDate,
    pub(super) focus: Segment,
    /// Digits typed into the focused segment; `None` means show the date.
    pub(super) draft: Option<String>,
}

impl DateEditor {
    pub(super) fn new(date: NaiveDate) -> Self {
        Self {
            date,
            focus: Segment::Year,
            draft: None,
        }
    }

    pub(super) fn on_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Tab if key.modifiers.is_empty() => {
                self.abandon_draft();
                self.focus = self.focus.next();
            }
            KeyCode::BackTab => {
                self.abandon_draft();
                self.focus = self.focus.previous();
            }
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.abandon_draft();
                self.focus = self.focus.previous();
            }
            KeyCode::Char('n') if key.modifiers.is_empty() => {
                self.abandon_draft();
                self.bump(1);
            }
            KeyCode::Char('p') if key.modifiers.is_empty() => {
                self.abandon_draft();
                self.bump(-1);
            }
            KeyCode::Char(c) if key.modifiers.is_empty() && c.is_ascii_digit() => {
                self.type_digit(c);
            }
            KeyCode::Backspace if key.modifiers.is_empty() => {
                if let Some(draft) = &mut self.draft {
                    draft.pop();
                }
            }
            _ => {}
        }
    }

    fn abandon_draft(&mut self) {
        self.draft = None;
    }

    fn type_digit(&mut self, digit: char) {
        let width = self.focus.width();
        // Starting a draft from `None` wipes the committed segment on screen.
        let draft = self.draft.get_or_insert_with(String::new);
        if draft.len() >= width {
            return;
        }
        draft.push(digit);
        if draft.len() == width {
            self.commit_draft_and_advance();
        }
    }

    fn commit_draft_and_advance(&mut self) {
        let Some(draft) = self.draft.take() else {
            return;
        };
        if let Some(date) = apply_segment(self.date, self.focus, &draft) {
            self.date = date;
        }
        self.focus = self.focus.next();
    }

    fn bump(&mut self, delta: i32) {
        self.date = match self.focus {
            Segment::Year => bump_year(self.date, delta),
            Segment::Month => bump_month(self.date, delta),
            Segment::Day => bump_day(self.date, delta),
        };
    }

    fn display_segment(&self, segment: Segment) -> String {
        let width = segment.width();
        if self.focus == segment {
            if let Some(draft) = &self.draft {
                let mut text = draft.clone();
                while text.len() < width {
                    text.push(' ');
                }
                return text;
            }
        }
        match segment {
            Segment::Year => format!("{:04}", self.date.year()),
            Segment::Month => format!("{:02}", self.date.month()),
            Segment::Day => format!("{:02}", self.date.day()),
        }
    }
}

fn apply_segment(date: NaiveDate, segment: Segment, draft: &str) -> Option<NaiveDate> {
    match segment {
        Segment::Year => {
            let year = draft.parse::<i32>().ok()?;
            clamp_ymd(year, date.month(), date.day())
        }
        Segment::Month => {
            let month = draft.parse::<u32>().ok()?.clamp(1, 12);
            clamp_ymd(date.year(), month, date.day())
        }
        Segment::Day => {
            let month = date.month();
            let year = date.year();
            let max = days_in_month(year, month)?;
            let day = draft.parse::<u32>().ok()?.clamp(1, max);
            NaiveDate::from_ymd_opt(year, month, day)
        }
    }
}

fn days_in_month(year: i32, month: u32) -> Option<u32> {
    let (y, m) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let first_next = NaiveDate::from_ymd_opt(y, m, 1)?;
    Some(first_next.pred_opt()?.day())
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
        (1..=day)
            .rev()
            .find_map(|d| NaiveDate::from_ymd_opt(year, month, d))
    })
}

fn center_text(text: &str, width: usize) -> String {
    let pad = width.saturating_sub(text.chars().count());
    let left = pad / 2;
    let right = pad - left;
    format!("{}{}{}", " ".repeat(left), text, " ".repeat(right))
}

/// `cal`-style month grid for `date`, without the year in the title.
pub(super) fn month_preview_lines(date: NaiveDate) -> Vec<String> {
    let title = center_text(&date.format("%B").to_string(), CAL_WIDTH);
    let mut lines = vec![title, "Su Mo Tu We Th Fr Sa".into()];
    let first = NaiveDate::from_ymd_opt(date.year(), date.month(), 1)
        .expect("date picker month always has a first day");
    let mut day = 1u32;
    let last = days_in_month(date.year(), date.month()).unwrap_or(28);
    let mut week = vec!["  ".to_string(); first.weekday().num_days_from_sunday() as usize];
    while day <= last {
        week.push(format!("{day:2}"));
        if week.len() == 7 {
            lines.push(week.join(" "));
            week.clear();
        }
        day += 1;
    }
    if !week.is_empty() {
        lines.push(week.join(" "));
    }
    lines
}

fn week_count(date: NaiveDate) -> u16 {
    month_preview_lines(date).len().saturating_sub(2) as u16
}

/// Centered stack: date picker on top, month preview beneath.
pub(super) fn stack_rects(area: Rect, date: NaiveDate) -> (Rect, Rect) {
    let width = area.width.min(PANEL_WIDTH);
    let preview_height = PREVIEW_CHROME + week_count(date);
    let total_height = PICKER_HEIGHT + GAP + preview_height;
    let height = area.height.min(total_height);
    let stack = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let picker_height = stack.height.min(PICKER_HEIGHT);
    let picker = Rect::new(stack.x, stack.y, stack.width, picker_height);
    let preview_top = stack.y.saturating_add(picker_height).saturating_add(GAP);
    let preview_height = stack
        .height
        .saturating_sub(picker_height.saturating_add(GAP));
    let preview = Rect::new(
        stack.x,
        preview_top.min(stack.y + stack.height),
        stack.width,
        preview_height,
    );
    (picker, preview)
}

fn segment_spans(editor: &DateEditor, theme: &Theme) -> Line<'static> {
    let gap = Span::styled(" ", Style::default().fg(theme.fg));
    Line::from(vec![
        segment_span(
            &editor.display_segment(Segment::Year),
            editor.focus == Segment::Year,
            theme,
        ),
        gap.clone(),
        segment_span(
            &editor.display_segment(Segment::Month),
            editor.focus == Segment::Month,
            theme,
        ),
        gap,
        segment_span(
            &editor.display_segment(Segment::Day),
            editor.focus == Segment::Day,
            theme,
        ),
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

fn preview_paragraph(date: NaiveDate, theme: &Theme) -> Paragraph<'static> {
    let selected = date.day();
    let lines: Vec<Line<'static>> = month_preview_lines(date)
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            if index < 2 {
                return Line::styled(line, Style::default().fg(theme.fg));
            }
            // Week rows: " 1  2  3 ..." — highlight the selected day cell.
            let mut spans = Vec::new();
            for (cell_index, cell) in line.split(' ').enumerate() {
                if cell_index > 0 {
                    spans.push(Span::raw(" "));
                }
                let day = cell.trim().parse::<u32>().ok();
                let style = if day == Some(selected) {
                    theme.selection().add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.fg)
                };
                spans.push(Span::styled(cell.to_string(), style));
            }
            Line::from(spans)
        })
        .collect();
    Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center)
}

pub(super) fn draw(frame: &mut Frame, editor: &DateEditor) {
    let area = frame.area();
    let theme = theme::load(&Config::default().theme);
    let (picker, preview) = stack_rects(area, editor.date);
    let stack = Rect::new(
        picker.x,
        picker.y,
        picker.width,
        preview.y + preview.height - picker.y,
    );
    frame.render_widget(Clear, stack);

    let picker_block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" date ")
        .border_style(Style::default().fg(theme.muted))
        .style(Style::default().fg(theme.fg).bg(theme.bg));
    let picker_inner = picker_block.inner(picker);
    frame.render_widget(picker_block, picker);
    frame.render_widget(
        Paragraph::new(segment_spans(editor, &theme)).alignment(ratatui::layout::Alignment::Center),
        picker_inner,
    );

    if preview.height >= 3 {
        let preview_block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title(" preview ")
            .border_style(Style::default().fg(theme.muted))
            .style(Style::default().fg(theme.fg).bg(theme.bg));
        let preview_inner = preview_block.inner(preview);
        frame.render_widget(preview_block, preview);
        frame.render_widget(preview_paragraph(editor.date, &theme), preview_inner);
    }
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

    fn digit(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn starts_with_the_year_focused() {
        let editor = DateEditor::new(date(2026, 11, 2));
        assert_eq!(editor.focus, Segment::Year);
        assert_eq!(editor.date, date(2026, 11, 2));
        assert_eq!(editor.draft, None);
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
    fn typing_digits_replaces_the_segment_and_advances_when_full() {
        let mut editor = DateEditor::new(date(2026, 11, 2));
        editor.on_key(digit('1'));
        assert_eq!(editor.draft.as_deref(), Some("1"));
        assert_eq!(editor.date, date(2026, 11, 2));
        assert_eq!(editor.display_segment(Segment::Year), "1   ");

        editor.on_key(digit('9'));
        editor.on_key(digit('9'));
        editor.on_key(digit('9'));
        assert_eq!(editor.draft, None);
        assert_eq!(editor.date, date(1999, 11, 2));
        assert_eq!(editor.focus, Segment::Month);

        editor.on_key(digit('0'));
        editor.on_key(digit('3'));
        assert_eq!(editor.date, date(1999, 3, 2));
        assert_eq!(editor.focus, Segment::Day);

        editor.on_key(digit('0'));
        editor.on_key(digit('9'));
        assert_eq!(editor.date, date(1999, 3, 9));
        assert_eq!(editor.focus, Segment::Year);
    }

    #[test]
    fn typing_clamps_invalid_month_and_day() {
        let mut editor = DateEditor::new(date(2026, 1, 31));
        editor.focus = Segment::Month;
        editor.on_key(digit('1'));
        editor.on_key(digit('3'));
        assert_eq!(editor.date, date(2026, 12, 31));
        assert_eq!(editor.focus, Segment::Day);

        editor.on_key(digit('0'));
        editor.on_key(digit('0'));
        assert_eq!(editor.date.day(), 1);
    }

    #[test]
    fn tab_while_typing_discards_the_draft() {
        let mut editor = DateEditor::new(date(2026, 11, 2));
        editor.on_key(digit('1'));
        editor.on_key(digit('9'));
        editor.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(editor.draft, None);
        assert_eq!(editor.date, date(2026, 11, 2));
        assert_eq!(editor.focus, Segment::Month);
    }

    #[test]
    fn backspace_edits_the_draft() {
        let mut editor = DateEditor::new(date(2026, 11, 2));
        editor.on_key(digit('2'));
        editor.on_key(digit('0'));
        editor.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(editor.draft.as_deref(), Some("2"));
        editor.on_key(digit('0'));
        editor.on_key(digit('2'));
        editor.on_key(digit('5'));
        assert_eq!(editor.date, date(2025, 11, 2));
        assert_eq!(editor.focus, Segment::Month);
    }

    #[test]
    fn month_preview_matches_cal_without_the_year() {
        assert_eq!(
            month_preview_lines(date(2026, 11, 2)),
            [
                "      November      ",
                "Su Mo Tu We Th Fr Sa",
                " 1  2  3  4  5  6  7",
                " 8  9 10 11 12 13 14",
                "15 16 17 18 19 20 21",
                "22 23 24 25 26 27 28",
                "29 30",
            ]
        );
        assert_eq!(
            month_preview_lines(date(2026, 2, 14))[0],
            "      February      "
        );
        assert_eq!(
            month_preview_lines(date(2026, 2, 14))[2],
            " 1  2  3  4  5  6  7"
        );
    }

    #[test]
    fn stack_sits_in_the_center_of_the_viewport() {
        let area = Rect::new(0, 0, 80, 24);
        let (picker, preview) = stack_rects(area, date(2026, 11, 2));
        assert_eq!(picker.width, PANEL_WIDTH);
        assert_eq!(picker.height, PICKER_HEIGHT);
        assert_eq!(picker.x, (80 - PANEL_WIDTH) / 2);
        assert!(preview.y >= picker.y + picker.height);
        assert_eq!(preview.x, picker.x);
        assert_eq!(preview.width, picker.width);
    }

    #[test]
    fn draw_shows_the_date_picker_and_month_preview() {
        let editor = DateEditor {
            date: date(2026, 3, 9),
            focus: Segment::Month,
            draft: None,
        };
        let mut terminal = Terminal::new(TestBackend::new(40, 20)).unwrap();
        terminal.draw(|frame| draw(frame, &editor)).unwrap();
        let screen = format!("{:?}", terminal.backend().buffer());
        assert!(screen.contains("2026"), "{screen}");
        assert!(screen.contains("03"), "{screen}");
        assert!(screen.contains("09"), "{screen}");
        assert!(screen.contains("date"), "{screen}");
        assert!(screen.contains("preview"), "{screen}");
        assert!(screen.contains("March"), "{screen}");
        assert!(screen.contains("Su Mo Tu We Th Fr Sa"), "{screen}");
    }
}
