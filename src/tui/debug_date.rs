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
const DATE_ROWS: u16 = 1;
const GAP: u16 = 1; // blank row between date segments and calendar
const PREVIEW_HEADER: u16 = 2; // month title + weekday header
const BORDER: u16 = 2; // top + bottom border rows

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

/// Board overlay: edit a selected task's due date.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DateEdit {
    pub(super) list_id: String,
    pub(super) task_id: String,
    pub(super) editor: DateEditor,
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
            lines.push(format_week(&week));
            week.clear();
        }
        day += 1;
    }
    if !week.is_empty() {
        while week.len() < 7 {
            week.push("  ".into());
        }
        lines.push(format_week(&week));
    }
    lines
}

fn format_week(cells: &[String]) -> String {
    debug_assert_eq!(cells.len(), 7);
    cells.join(" ")
}

fn week_count(date: NaiveDate) -> u16 {
    month_preview_lines(date).len().saturating_sub(2) as u16
}

/// Centered panel that holds the date segments and month calendar together.
pub(super) fn panel_rect(area: Rect, date: NaiveDate) -> Rect {
    let width = area.width.min(PANEL_WIDTH);
    let content = DATE_ROWS + GAP + PREVIEW_HEADER + week_count(date);
    let height = area.height.min(BORDER + content);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
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
            // Fixed columns: each day is two cells, separated by one space.
            // Do not split on ' ' — single-digit days are " 1" and would break.
            let mut spans = Vec::new();
            for cell_index in 0..7 {
                if cell_index > 0 {
                    spans.push(Span::raw(" "));
                }
                let start = cell_index * 3;
                let cell = line.get(start..start + 2).unwrap_or("  ");
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
    // Left-aligned so short or padded weeks stay under Su..Sa; the panel is
    // already centered as a whole by panel_rect.
    Paragraph::new(lines)
}

/// Draw the date panel centered in `area` using `theme`.
pub(super) fn draw_panel(frame: &mut Frame, area: Rect, editor: &DateEditor, theme: &Theme) {
    let panel = panel_rect(area, editor.date);
    frame.render_widget(Clear, panel);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" date ")
        .border_style(Style::default().fg(theme.muted))
        .style(Style::default().fg(theme.fg).bg(theme.bg));
    let inner = block.inner(panel);
    frame.render_widget(block, panel);

    if inner.height == 0 {
        return;
    }
    let date_area = Rect::new(inner.x, inner.y, inner.width, DATE_ROWS.min(inner.height));
    frame.render_widget(
        Paragraph::new(segment_spans(editor, theme)).alignment(ratatui::layout::Alignment::Center),
        date_area,
    );

    let preview_top = inner
        .y
        .saturating_add(DATE_ROWS)
        .saturating_add(GAP)
        .min(inner.y + inner.height);
    let preview_height = inner.height.saturating_sub(DATE_ROWS.saturating_add(GAP));
    if preview_height > 0 {
        let preview = Rect::new(inner.x, preview_top, inner.width, preview_height);
        frame.render_widget(preview_paragraph(editor.date, theme), preview);
    }
}

pub(super) fn draw(frame: &mut Frame, editor: &DateEditor) {
    let theme = theme::load(&Config::default().theme);
    draw_panel(frame, frame.area(), editor, &theme);
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
                "29 30               ",
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
        // November 2026 has 5 week rows → panel is borders + date + gap + header + weeks.
        let panel = panel_rect(area, date(2026, 11, 2));
        assert_eq!(panel.width, PANEL_WIDTH);
        assert_eq!(panel.height, 2 + DATE_ROWS + GAP + 2 + 5);
        assert_eq!(panel.x, (80 - PANEL_WIDTH) / 2);
        assert_eq!(panel.y, (24 - panel.height) / 2);
    }

    #[test]
    fn draw_shows_one_panel_with_date_and_month_preview() {
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
        assert!(
            !screen.contains("preview"),
            "date and calendar share one border; no separate preview title: {screen}"
        );
        assert!(screen.contains("March"), "{screen}");
        assert!(screen.contains("Su Mo Tu We Th Fr Sa"), "{screen}");
    }

    #[test]
    fn preview_week_rows_keep_weekday_columns() {
        // September 2026 starts on Tuesday, so the 1st is under Tu.
        let lines = month_preview_lines(date(2026, 9, 1));
        assert_eq!(lines[1], "Su Mo Tu We Th Fr Sa");
        assert_eq!(lines[2], "       1  2  3  4  5");
        assert_eq!(lines[2].len(), CAL_WIDTH);
        // Last week must stay left-aligned under Su/Mo, not centered.
        let last = lines.last().unwrap();
        assert!(last.starts_with("27 28 29 30"), "{last:?}");
        assert_eq!(last.len(), CAL_WIDTH);
    }
}
