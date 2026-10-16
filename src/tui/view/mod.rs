//! Drawing. Lays out the header, board, overlay, and status bar from app
//! state.

mod columns;
mod overlay;
mod shelf;
mod status;
pub(super) mod text;

use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Block,
    Frame,
};

use super::app::App;

impl App {
    pub(super) fn bg(&self, color: Color) -> Color {
        if self.transparent {
            Color::Reset
        } else {
            color
        }
    }

    pub(in crate::tui) fn draw(&mut self, f: &mut Frame) {
        self.edit_width = 0;
        let t = self.theme.clone();
        let base = Style::default().fg(t.fg).bg(self.bg(t.bg));
        f.render_widget(Block::default().style(base), f.area());

        let [header, body, status] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(f.area());

        let mut head = vec![
            Span::styled(" Tasks ", t.badge().bold()),
            Span::styled(format!("  theme: {}", t.name), Style::default().fg(t.muted)),
        ];
        if self.pending_operation.is_some() {
            head.push(Span::styled("  saving…", Style::default().fg(t.yellow)));
        } else if self.loading() {
            head.push(Span::styled("  syncing…", Style::default().fg(t.yellow)));
        }
        f.render_widget(Line::from(head), header);

        self.draw_board(f, body, &t);
        self.draw_picker(f, body, &t);

        self.draw_status(f, status, &t);
    }
}

#[cfg(test)]
mod tests {
    use crate::cache;
    use crate::config::Config;
    use crate::keys::BindingConfig;
    use crate::model::{Status, Task, TaskList};
    use crate::tui::app::{App, Interaction, Shelf};
    use crate::tui::commands::ParentPick;
    use crate::tui::editor::FieldId;
    use crate::tui::picker::{Picker, PickerRow};
    use crate::tui::sync::SyncState;
    use ratatui::{
        backend::TestBackend,
        crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
        Terminal,
    };
    use std::sync::mpsc;

    fn render_app(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..h)
            .flat_map(|y| {
                (0..w)
                    .map(move |x| buffer[(x, y)].symbol())
                    .chain(std::iter::once("\n"))
            })
            .collect()
    }

    fn one_list_app() -> App {
        let (tx, _rx) = mpsc::channel();
        App::new(
            tx,
            cache::Cache {
                lists: vec![cache::CachedList {
                    list: TaskList {
                        id: "list".into(),
                        title: "Underlying board".into(),
                        ..Default::default()
                    },
                    tasks: vec![Task {
                        id: "task".into(),
                        title: "Underlying task".into(),
                        status: Status::NeedsAction,
                        ..Default::default()
                    }],
                }],
            },
            Config::default(),
        )
    }

    /// Render a board of `(list title, task titles)` and return the screen.
    fn screen(w: u16, h: u16, lists: &[(&str, &[&str])], wrap_titles: bool) -> Vec<String> {
        let (tx, _rx) = mpsc::channel();
        let cached = cache::Cache {
            lists: lists
                .iter()
                .enumerate()
                .map(|(i, (name, titles))| cache::CachedList {
                    list: TaskList {
                        id: format!("l{i}"),
                        title: (*name).into(),
                        ..Default::default()
                    },
                    tasks: titles
                        .iter()
                        .enumerate()
                        .map(|(j, title)| Task {
                            id: format!("l{i}t{j}"),
                            title: (*title).into(),
                            position: format!("{j:09}"),
                            status: Status::NeedsAction,
                            ..Default::default()
                        })
                        .collect(),
                })
                .collect(),
        };
        let mut app = App::new(
            tx,
            cached,
            Config {
                wrap_titles,
                ..Config::default()
            },
        );
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn a_wrapped_line_never_starts_on_a_stray_space() {
        // Breaking on a run of spaces hands the next line a leading one, which
        // shows up as a one-column indent under the rest of the title.
        let title = "aaaaaaaaaaaaaaaaaaaaaaaa  gamma";
        // 30 wide puts the break between the two spaces.
        let rows = screen(30, 8, &[("One", &[title])], true);
        let col = |needle: &str| {
            rows.iter()
                .find_map(|r| r.find(needle).map(|b| r[..b].chars().count()))
                .unwrap_or_else(|| panic!("{needle} is not on screen: {rows:#?}"))
        };
        assert_eq!(col("gamma"), col("aaaa"));
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(row.chars().count(), 30, "row {i} is {row:?}");
        }
    }

    #[test]
    fn status_hints_use_the_active_keymap() {
        let mut app = one_list_app();
        let mut config = Config::default();
        config.keys.create_task = Some(BindingConfig::one("n"));
        let (keymap, diagnostics) = config.keymap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        app.config = config;
        app.keymap = keymap;
        app.sync = SyncState::Ready;

        let screen = render_app(&mut app, 160, 8);
        assert!(screen.contains("n new"), "{screen}");
        assert!(!screen.contains("c new"), "{screen}");
    }

    /// A control character reaching a cell is not cosmetic: printing `\r`
    /// walks the terminal cursor back to column 0 and shifts the rest of the
    /// row. Clipping hid them past a column's edge; wrapping does not.
    #[test]
    fn control_characters_never_reach_the_screen() {
        let rows = screen(
            34,
            14,
            &[
                (
                    "One",
                    &[
                        "print \r and bring with you to the hospital",
                        "bell\u{7}here",
                    ],
                ),
                ("Two\r", &["fine"]),
            ],
            true,
        );
        for (y, row) in rows.iter().enumerate() {
            for c in row.chars() {
                assert!(!c.is_ascii_control(), "{c:?} on row {y}: {row:?}");
            }
            assert_eq!(row.chars().count(), 34, "row {y} is {row:?}");
        }
        assert!(rows.iter().any(|r| r.contains("and bring")), "{rows:#?}");
    }

    #[test]
    fn every_row_of_the_board_is_the_full_width() {
        let long = "Complete Birth Certificate Worksheet as much as possible, print and bring";
        let rows = screen(
            62,
            20,
            &[
                (
                    "One",
                    &["Extend irrigation to the outer plants", "Baby go bag prep"],
                ),
                ("Two", &[long, "Irrigation for box"]),
            ],
            true,
        );
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(row.chars().count(), 62, "row {i} is {row:?}");
        }
    }

    #[test]
    fn shelf_keeps_notes_and_updated_visible_with_many_links() {
        let (tx, _rx) = mpsc::channel();
        let task = Task {
            id: "task".into(),
            title: "Task".into(),
            notes: Some("Important notes".into()),
            status: Status::NeedsAction,
            links: (0..12)
                .map(|index| crate::model::Link {
                    description: format!("Link {index}"),
                    link: format!("https://example.test/{index}"),
                })
                .collect(),
            ..Default::default()
        };
        let cached = cache::Cache {
            lists: vec![cache::CachedList {
                list: TaskList {
                    id: "list".into(),
                    title: "List".into(),
                    ..Default::default()
                },
                tasks: vec![task],
            }],
        };
        let mut app = App::new(tx, cached, Config::default());
        app.shelf = Shelf::Open(FieldId::Title);
        let mut terminal = Terminal::new(TestBackend::new(90, 18)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let screen: String = (0..18)
            .flat_map(|y| {
                (0..90)
                    .map(move |x| buffer[(x, y)].symbol())
                    .chain(std::iter::once("\n"))
            })
            .collect();

        assert!(screen.contains("Notes"), "{screen}");
        assert!(screen.contains("Important notes"), "{screen}");
        assert!(screen.contains("Updated"), "{screen}");
        assert!(screen.contains("More"), "{screen}");
    }

    #[test]
    fn narrow_terminal_uses_a_visible_full_width_editor() {
        let (tx, _rx) = mpsc::channel();
        let cached = cache::Cache {
            lists: vec![cache::CachedList {
                list: TaskList {
                    id: "list".into(),
                    title: "List".into(),
                    ..Default::default()
                },
                tasks: Vec::new(),
            }],
        };
        let mut app = App::new(tx, cached, Config::default());
        app.sync = SyncState::Ready;
        app.start_create(false);
        let mut terminal = Terminal::new(TestBackend::new(36, 10)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();

        assert!(app.edit_width > 0);
        let buffer = terminal.backend().buffer();
        let screen: String = (0..10)
            .flat_map(|y| {
                (0..36)
                    .map(move |x| buffer[(x, y)].symbol())
                    .chain(std::iter::once("\n"))
            })
            .collect();
        assert!(screen.contains("New task"), "{screen}");
    }

    #[test]
    fn small_terminal_messages_name_the_configured_keys() {
        let mut config = Config::default();
        config.keys.cancel_edit = Some(BindingConfig::one("ctrl+g"));
        config.keys.close_shelf = Some(BindingConfig::one("ctrl+o"));
        let (keymap, diagnostics) = config.keymap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");

        let mut creating = one_list_app();
        creating.keymap = keymap.clone();
        creating.sync = SyncState::Ready;
        creating.start_create(false);
        let screen = render_app(&mut creating, 40, 6);
        assert!(
            screen.contains("Terminal too small; ctrl+g cancels"),
            "{screen}"
        );

        let mut details = one_list_app();
        details.keymap = keymap;
        details.shelf = Shelf::Open(FieldId::Title);
        let screen = render_app(&mut details, 50, 8);
        assert!(
            screen.contains("Terminal too small; ctrl+o closes details"),
            "{screen}"
        );
    }

    #[test]
    fn every_overlay_renders_on_a_cleared_bounded_surface() {
        let mut browse = one_list_app();
        browse.interaction = Interaction::Browse(Picker::new(vec![PickerRow::new(
            "Example".into(),
            Some("https://example.test".into()),
            "https://example.test".into(),
        )]));
        let screen = render_app(&mut browse, 50, 14);
        assert!(screen.contains("Browse Link"), "{screen}");
        assert!(screen.contains("Example"), "{screen}");

        let mut movement = one_list_app();
        movement.interaction = Interaction::Move(Picker::new(vec![PickerRow::new(
            "Destination".into(),
            None,
            "destination".into(),
        )]));
        let screen = render_app(&mut movement, 50, 14);
        assert!(screen.contains("Move"), "{screen}");
        assert!(screen.contains("Destination"), "{screen}");

        let mut parent = one_list_app();
        parent.interaction = Interaction::Parent(ParentPick {
            list_id: "list".into(),
            task_id: "task".into(),
            picker: Picker::new(vec![PickerRow::new(
                "Parent task".into(),
                None,
                "parent".into(),
            )]),
        });
        let screen = render_app(&mut parent, 50, 14);
        assert!(screen.contains("Select Parent"), "{screen}");
        assert!(screen.contains("Parent task"), "{screen}");

        let mut help = one_list_app();
        help.on_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        let screen = render_app(&mut help, 50, 20);
        assert!(screen.contains("Keyboard"), "{screen}");
        assert!(screen.contains("new task"), "{screen}");
    }

    #[test]
    fn extremely_narrow_terminals_render_without_panics_or_out_of_bounds_rows() {
        for (width, height) in [(1, 1), (2, 3), (8, 4), (12, 6)] {
            let mut app = one_list_app();
            let screen = render_app(&mut app, width, height);
            assert_eq!(
                screen.lines().count(),
                usize::from(height),
                "{width}x{height}: {screen:?}"
            );
            assert!(screen
                .lines()
                .all(|line| line.chars().count() <= width.into()));
        }
    }

    #[test]
    fn editing_renders_cjk_emoji_combining_text_and_sanitizes_controls() {
        let mut app = one_list_app();
        app.sync = SyncState::Ready;
        app.start_create(false);
        render_app(&mut app, 60, 10);
        for character in "任务🙂e\u{301}\u{7}".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }

        let screen = render_app(&mut app, 60, 10);

        assert!(screen.contains('任') && screen.contains('务'), "{screen}");
        assert!(screen.contains('🙂'), "{screen}");
        assert!(screen.contains("e\u{301}"), "{screen}");
        assert!(!screen
            .chars()
            .any(|character| { character.is_ascii_control() && character != '\n' }));
    }
}
