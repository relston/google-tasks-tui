mod theme;

use std::time::{Duration, Instant};

use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyEventKind},
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, List, ListItem, ListState, Paragraph},
    DefaultTerminal, Frame,
};
use theme::Theme;

const THEME_POLL: Duration = Duration::from_millis(500);

struct App {
    theme: Theme,
    fingerprint: theme::Fingerprint,
    transparent: bool,
    items: Vec<(&'static str, &'static str)>,
    list: ListState,
    quit: bool,
}

impl App {
    fn new() -> Self {
        Self {
            theme: theme::load(),
            fingerprint: theme::fingerprint(),
            transparent: false,
            items: vec![
                ("ok", "Build finished"),
                ("warn", "3 deprecated calls"),
                ("err", "Connection refused"),
                ("info", "Watching for theme changes"),
                ("ok", "Tests passed"),
            ],
            list: ListState::default().with_selected(Some(0)),
            quit: false,
        }
    }

    /// Reload the palette only when the fingerprint changes (cheap stat calls).
    fn check_theme(&mut self) {
        let fp = theme::fingerprint();
        if fp != self.fingerprint {
            self.fingerprint = fp;
            self.theme = theme::load();
        }
    }

    fn bg(&self, c: Color) -> Color {
        if self.transparent { Color::Reset } else { c }
    }

    fn on_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('j') | KeyCode::Down => self.list.select_next(),
            KeyCode::Char('k') | KeyCode::Up => self.list.select_previous(),
            KeyCode::Char('t') => self.transparent = !self.transparent,
            KeyCode::Char('r') => self.theme = theme::load(),
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame) {
        let t = self.theme.clone();
        let base = Style::default().fg(t.fg).bg(self.bg(t.bg));
        f.render_widget(Block::default().style(base), f.area());

        let [header, body, status] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(f.area());

        f.render_widget(
            Line::from(vec![
                Span::styled(" omarchy-tui ", Style::default().fg(t.bg).bg(t.accent).bold()),
                Span::styled(format!("  theme: {}", t.name), Style::default().fg(t.muted)),
            ]),
            header,
        );

        let items: Vec<ListItem> = self
            .items
            .iter()
            .map(|(kind, msg)| {
                let c = match *kind {
                    "ok" => t.green,
                    "warn" => t.yellow,
                    "err" => t.red,
                    _ => t.blue,
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{kind:>5} "), Style::default().fg(c).bold()),
                    Span::raw(*msg),
                ]))
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(t.accent))
                    .title(" Events ")
                    .style(Style::default().bg(self.bg(t.surface))),
            )
            .highlight_style(
                Style::default().fg(t.sel_fg).bg(t.sel_bg).add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▌ ");
        f.render_stateful_widget(list, body, &mut self.list);

        f.render_widget(
            Paragraph::new(" j/k move · t transparent · r reload · q quit")
                .style(Style::default().fg(t.muted)),
            status,
        );
    }
}

fn run(mut term: DefaultTerminal) -> std::io::Result<()> {
    let mut app = App::new();
    let mut last_poll = Instant::now();
    while !app.quit {
        term.draw(|f| app.draw(f))?;
        // Block on input, but wake periodically to check for theme switches.
        if event::poll(THEME_POLL)? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    app.on_key(k.code);
                }
            }
        }
        if last_poll.elapsed() >= THEME_POLL {
            app.check_theme();
            last_poll = Instant::now();
        }
    }
    Ok(())
}

fn main() -> std::io::Result<()> {
    let term = ratatui::init();
    let res = run(term);
    ratatui::restore();
    res
}
