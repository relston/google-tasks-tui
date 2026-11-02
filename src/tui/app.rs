//! TUI state: the board, columns, the current interaction, and shutdown.
//! Behavior lives in the sibling modules as further `impl App` blocks.

use std::sync::mpsc::Sender;

use ratatui::layout::Rect;

use crate::board::Board;
use crate::cache;
use crate::config::Config;
use crate::keys::{Action, Keymap};
use crate::paths::Paths;
use crate::theme::{self, Theme};

use super::columns::{visible_rows, Column};
use super::commands::{ParentPick, Pending};
use super::debug_date::DateEdit;
use super::editor::{Edit, FieldId};
use super::mutation::PendingOperation;
use super::picker::Picker;
use super::sync::{LoadState, SyncState};
use super::worker::{Cmd, Msg};

/// The details shelf beside the focused column, and the field the keyboard
/// is on while it is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shelf {
    Closed,
    Open(FieldId),
}

impl Shelf {
    pub(super) fn is_open(self) -> bool {
        matches!(self, Self::Open(_))
    }
}

/// Shutdown progress. Only a quit that waited for a save joins the worker
/// afterwards, so that save is never cut off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QuitState {
    Running,
    /// Quit was requested while a save was in flight.
    Waiting,
    /// The awaited save landed.
    Drained,
    /// Nothing was pending, or the user forced it.
    Now,
}

pub(super) enum Interaction {
    Idle,
    Editing(Edit),
    DateEdit(DateEdit),
    ConfirmDelete(Pending),
    Browse(Picker),
    Move(Picker),
    Parent(ParentPick),
    Help(Picker),
}

impl Interaction {
    pub(super) fn picker(&self) -> Option<&Picker> {
        match self {
            Interaction::Browse(p) | Interaction::Move(p) | Interaction::Help(p) => Some(p),
            Interaction::Parent(parent) => Some(&parent.picker),
            _ => None,
        }
    }

    pub(super) fn picker_mut(&mut self) -> Option<&mut Picker> {
        match self {
            Interaction::Browse(p) | Interaction::Move(p) | Interaction::Help(p) => Some(p),
            Interaction::Parent(parent) => Some(&mut parent.picker),
            _ => None,
        }
    }

    /// The heading drawn in the overlay's border.
    pub(super) fn picker_title(&self) -> &'static str {
        match self {
            Interaction::Browse(_) => " Browse Link ",
            Interaction::Move(_) => " Move ",
            Interaction::Parent(_) => " Select Parent ",
            _ => " Keyboard ",
        }
    }
}

pub(super) struct App {
    pub(super) config: Config,
    pub(super) keymap: Keymap,
    pub(super) theme: Theme,
    pub(super) fingerprint: theme::Fingerprint,
    pub(super) transparent: bool,
    /// When false, completed tasks stay on the board but leave the columns.
    pub(super) show_completed: bool,
    pub(super) board: Board,
    pub(super) paths: Paths,
    pub(super) cache_meta: cache::Meta,
    pub(super) columns: Vec<Column>,
    pub(super) focus: usize,
    /// The terminal area, recorded by the event loop so input can use the
    /// same layout drawing does.
    pub(super) viewport: Rect,
    pub(super) shelf: Shelf,
    pub(super) interaction: Interaction,
    /// Identifies the only refresh whose responses may update the board.
    pub(super) refresh_generation: u64,
    pub(super) sync: SyncState,
    pub(super) next_operation: u64,
    pub(super) pending_operation: Option<PendingOperation>,
    pub(super) error: Option<String>,
    /// A one-off confirmation, cleared by the next keypress.
    pub(super) notice: Option<String>,
    pub(super) cmds: Sender<Cmd>,
    pub(super) quit: QuitState,
}

impl App {
    /// A test app whose files live in a fresh temporary directory.
    #[cfg(test)]
    pub(super) fn new(cmds: Sender<Cmd>, cached: cache::Cache, config: Config) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gtt-app-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        Self::new_with_storage(
            cmds,
            cached,
            config,
            Paths {
                credentials: root.join("credentials.json"),
                config_dir: root,
            },
            cache::Meta {
                account: String::new(),
                generation: 0,
            },
            None,
        )
    }

    pub(super) fn new_with_storage(
        cmds: Sender<Cmd>,
        cached: cache::Cache,
        config: Config,
        paths: Paths,
        cache_meta: cache::Meta,
        warning: Option<String>,
    ) -> Self {
        let (keymap, key_diagnostics) = config.keymap();
        let key_warning = (!key_diagnostics.is_empty()).then(|| key_diagnostics.join("; "));
        let warning = match (warning, key_warning) {
            (Some(existing), Some(keys)) => Some(format!("{existing}; {keys}")),
            (warning, None) | (None, warning) => warning,
        };
        // Paint the last known board straight away; the sync replaces it.
        let board = Board::from_cache(cached);
        let columns = board
            .lists()
            .map(|list| {
                let rows = visible_rows(&board, &list.id, true);
                Column::new(list.clone(), rows, LoadState::Cached)
            })
            .collect();
        let theme = theme::load(&config.theme);
        let mut app = Self {
            config,
            keymap,
            theme,
            fingerprint: theme::fingerprint(),
            transparent: false,
            show_completed: true,
            board,
            paths,
            cache_meta,
            columns,
            focus: 0,
            viewport: Rect::default(),
            shelf: Shelf::Closed,
            interaction: Interaction::Idle,
            refresh_generation: 0,
            sync: SyncState::Cached,
            next_operation: 0,
            pending_operation: None,
            error: None,
            notice: None,
            cmds,
            quit: QuitState::Running,
        };
        app.begin_refresh(warning);
        app
    }

    pub(super) fn edit(&self) -> Option<&Edit> {
        match &self.interaction {
            Interaction::Editing(edit) => Some(edit),
            _ => None,
        }
    }

    pub(super) fn edit_mut(&mut self) -> Option<&mut Edit> {
        match &mut self.interaction {
            Interaction::Editing(edit) => Some(edit),
            _ => None,
        }
    }

    pub(super) fn is_creating(&self) -> bool {
        self.edit().is_some_and(Edit::is_new)
    }

    pub(super) fn should_exit(&self) -> bool {
        matches!(self.quit, QuitState::Drained | QuitState::Now)
    }

    pub(super) fn request_quit(&mut self) {
        self.quit = if self.quit == QuitState::Running && self.pending_operation.is_some() {
            let quit = self.keymap.labels_for(Action::Quit).join("/");
            self.notice = Some(format!(
                "waiting for pending save · {quit} again to force quit"
            ));
            QuitState::Waiting
        } else {
            QuitState::Now
        };
    }

    /// Reload the palette only when the fingerprint changes (cheap stat calls).
    pub(super) fn check_theme(&mut self) {
        let fp = theme::fingerprint();
        if fp != self.fingerprint {
            self.fingerprint = fp;
            self.theme = theme::load(&self.config.theme);
        }
    }

    pub(super) fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Lists(generation, lists) => self.lists_loaded(generation, lists),
            Msg::Tasks(generation, list_id, tasks) => self.tasks_loaded(generation, list_id, tasks),
            Msg::Finished { id, result } => self.finish(id, result),
        }
        self.settle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::test_support::*;

    #[test]
    fn test_apps_never_touch_the_real_config_directory() {
        let (first, _) = app_with_lists(&["a"]);
        let (second, _) = app_with_lists(&["a"]);
        assert!(first.paths.config_dir.starts_with(std::env::temp_dir()));
        assert_ne!(first.paths.config_dir, Paths::from_env().config_dir);
        assert_ne!(first.paths.config_dir, second.paths.config_dir);
    }

    #[test]
    fn quit_waits_for_writes_and_a_second_quit_forces_it() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        pending(&mut app);

        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(
            !app.should_exit(),
            "the first quit must wait for the pending write"
        );

        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(
            app.should_exit(),
            "the second quit is the explicit force-quit path"
        );
    }

    #[test]
    fn the_waiting_quit_notice_names_the_configured_quit_key() {
        let mut config = Config::default();
        config
            .keys
            .insert("quit".into(), BindingConfig::one("ctrl+x"));
        let (mut app, _cmds) = app_with_lists_and_config(&["a"], config);
        pending(&mut app);

        app.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));

        assert!(!app.should_exit());
        assert!(app
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("ctrl+x again to force quit")));
    }

    #[test]
    fn a_waiting_quit_finishes_when_the_last_write_returns() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        let operation = pending(&mut app);

        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!app.should_exit());
        app.on_msg(refused(operation));

        assert!(app.pending_operation.is_none());
        assert!(app.should_exit());
    }

    #[test]
    fn worker_disconnect_does_not_hang_a_waiting_quit() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        pending(&mut app);
        app.quit = QuitState::Waiting;

        app.worker_disconnected();

        assert!(app.pending_operation.is_none());
        assert!(app.should_exit());
        assert_eq!(app.sync, SyncState::Failed);
        assert!(app
            .error
            .as_deref()
            .is_some_and(|e| e.contains("worker stopped")));
    }

    #[test]
    fn a_waiting_quit_also_waits_for_queued_reorder_presses() {
        let (mut app, commands) = app_with_lists(&["a"]);
        let _ = commands.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        reorder_list(&mut app, &["a", "b", "c"]);

        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
        let (operation, _) = sent(&commands);
        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT));
        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!app.should_exit());

        app.on_msg(reordered(operation, &["b", "a", "c"]));
        assert!(
            !app.should_exit(),
            "the queued move is still a pending write"
        );
        let (operation, _) = sent(&commands);

        app.on_msg(reordered(operation, &["b", "c", "a"]));
        assert!(app.should_exit());
    }

    #[test]
    fn invalid_keybinding_diagnostics_survive_startup_refresh() {
        let mut config = Config::default();
        config
            .keys
            .insert("help".into(), BindingConfig::one("not-a-key"));
        let (app, _commands) = app_with_lists_and_config(&["a"], config);

        assert!(app
            .error
            .as_deref()
            .is_some_and(|error| error.contains("invalid keybinding")));
    }
}
