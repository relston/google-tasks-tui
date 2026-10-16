//! Key dispatch: resolve a key in the active context, then run its action.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::keys::{Action, BindingContext};

use super::app::{App, Interaction, QuitState, Shelf};
use super::editor::FieldId;
use super::picker::{picker_key, Picker, PickerKey};

pub(super) fn no_modifiers(modifiers: KeyModifiers) -> bool {
    modifiers.is_empty()
}

/// A printable key the user typed, with or without shift held.
pub(super) fn typed(modifiers: KeyModifiers) -> bool {
    no_modifiers(modifiers) || modifiers == KeyModifiers::SHIFT
}

impl App {
    pub(super) fn on_key(&mut self, k: KeyEvent) {
        // A notice is for the keypress that earned it, and no longer.
        self.notice = None;
        if self.quit != QuitState::Running {
            if self.keymap.resolve(BindingContext::Board, &k) == Some(Action::Quit) {
                self.request_quit();
            }
            return;
        }
        if matches!(self.interaction, Interaction::Editing(_)) {
            self.edit_key(k);
            return;
        }
        if matches!(self.interaction, Interaction::ConfirmDelete(_)) {
            // Only "y" deletes; every other key cancels.
            match self.keymap.resolve(BindingContext::Confirm, &k) {
                Some(Action::ConfirmDelete) => self.confirm_delete(),
                _ => self.interaction = Interaction::Idle,
            }
            return;
        }
        if self.interaction.picker().is_some() {
            let context = if self.interaction.picker().is_some_and(Picker::typing) {
                BindingContext::Filter
            } else {
                BindingContext::Picker
            };
            let action = self.keymap.resolve(context, &k);
            let close_on_help =
                matches!(self.interaction, Interaction::Help(_)) && action == Some(Action::Help);
            let outcome = match self.interaction.picker_mut() {
                Some(picker) if !close_on_help => picker_key(picker, action, k),
                _ => PickerKey::Close,
            };
            match outcome {
                PickerKey::Consumed => {}
                PickerKey::Close => self.interaction = Interaction::Idle,
                PickerKey::Accept => match self.interaction {
                    Interaction::Browse(_) => self.open_browse(false),
                    Interaction::Move(_) => self.confirm_move(),
                    Interaction::Parent(_) => self.confirm_parent(),
                    _ => self.interaction = Interaction::Idle,
                },
                PickerKey::AcceptAll => {
                    if matches!(self.interaction, Interaction::Browse(_)) {
                        self.open_browse(true);
                    }
                }
            }
            return;
        }
        let Some(action) = self.keymap.resolve(BindingContext::Board, &k) else {
            return;
        };
        match action {
            Action::Quit => self.request_quit(),
            Action::CloseShelf => self.shelf = Shelf::Closed,
            Action::PreviousList => self.focus = self.focus.saturating_sub(1),
            Action::NextList => {
                self.focus = (self.focus + 1).min(self.columns.len().saturating_sub(1))
            }
            Action::NextTask => self.step_selection(1),
            Action::PreviousTask => self.step_selection(-1),
            Action::FirstTask => self.step_selection(isize::MIN / 2),
            Action::LastTask => self.step_selection(isize::MAX / 2),
            Action::ToggleDetails => {
                self.shelf = if !self.shelf.is_open() && self.selected_task().is_some() {
                    Shelf::Open(FieldId::Title)
                } else {
                    Shelf::Closed
                };
            }
            Action::NextField | Action::PreviousField => {
                if let Shelf::Open(field) = self.shelf {
                    self.shelf = Shelf::Open(field.other());
                }
            }
            Action::Edit => self.start_edit(),
            Action::ToggleDone => self.toggle_done(),
            Action::ToggleCompleted => self.toggle_completed(),
            Action::DeleteTask => self.ask_delete(),
            Action::CreateTask => self.start_create(false),
            Action::CreateSubtask => self.start_create(true),
            Action::Refresh => self.refresh(),
            Action::ToggleTransparent => self.transparent = !self.transparent,
            Action::CopyTask => self.yank(),
            Action::BrowseLinks => self.start_browse(),
            Action::MoveTask => self.start_move(),
            Action::ReorderUp => self.reorder_task(false),
            Action::ReorderDown => self.reorder_task(true),
            Action::UnparentTask => self.unparent_task(),
            Action::ChooseParent => self.start_parent_pick(),
            Action::Help => self.start_help(),
            _ => {}
        }
    }

    /// Keys while editing a field. Enter saves, Esc discards; in notes,
    /// Shift+Enter (or Alt+Enter / Ctrl+J where the terminal can't report it)
    /// starts a new line.
    pub(super) fn edit_key(&mut self, k: KeyEvent) {
        let action = self.keymap.resolve(BindingContext::Editor, &k);
        if self.edit_width == 0 && action != Some(Action::CancelEdit) {
            let cancel = self.keymap.labels_for(Action::CancelEdit).join("/");
            self.error = Some(format!(
                "terminal too small to show the editor; {cancel} cancels"
            ));
            return;
        }
        let notes = self.edit().is_some_and(|e| e.field() == FieldId::Notes);
        match action {
            Some(Action::CancelEdit) => {
                self.interaction = Interaction::Idle;
                return;
            }
            Some(Action::SaveEdit) => {
                self.commit_edit();
                return;
            }
            _ => {}
        }
        let width = self.edit_width.max(1);
        let Some(e) = self.edit_mut() else { return };
        match action {
            Some(Action::InsertNewline) if notes => e.insert('\n'),
            Some(Action::DeleteWord) => e.kill_word(),
            Some(Action::ClearToLineStart) => e.kill_to_start(),
            Some(Action::CursorLineStart) => e.cursor = e.line_start(),
            Some(Action::CursorLineEnd) => e.cursor = e.line_end(),
            Some(Action::EditorBackspace) => e.backspace(),
            Some(Action::EditorDelete) => e.delete(),
            Some(Action::CursorLeft) => e.left(),
            Some(Action::CursorRight) => e.right(),
            Some(Action::CursorUp) if notes => e.step_visual_line(width, false),
            Some(Action::CursorDown) if notes => e.step_visual_line(width, true),
            None if matches!(k.code, KeyCode::Char(_))
                && (no_modifiers(k.modifiers) || k.modifiers == KeyModifiers::SHIFT) =>
            {
                let KeyCode::Char(c) = k.code else {
                    unreachable!()
                };
                e.insert(c)
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::test_support::*;

    #[test]
    fn navigation_remains_available_while_mutations_are_blocked() {
        let (mut app, cmds) = app_with_lists(&["a"]);
        let _ = cmds.recv().unwrap(); // startup refresh
        app.sync = SyncState::Ready;
        set_tasks(
            &mut app,
            "a",
            vec![
                task("a-one", None, "1", false),
                task("a-two", None, "2", false),
            ],
            "a-one",
        );

        app.send_patch(
            "a".into(),
            "a-one".into(),
            TaskPatch {
                title: Some("changed".into()),
                ..Default::default()
            },
            task("a-one", None, "1", false),
        );
        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

        assert_eq!(
            app.selected_task().map(|row| row.task.id.as_str()),
            Some("a-two")
        );
        assert!(app
            .error
            .as_deref()
            .is_some_and(|error| error.contains("current save")));
        assert_eq!(
            cmds.try_iter()
                .filter(|command| matches!(command, Cmd::Patch { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn modified_shortcuts_do_not_trigger_plain_commands() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        app.sync = SyncState::Ready;

        app.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(matches!(app.interaction, Interaction::Idle));

        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!app.should_exit());
    }

    #[test]
    fn configured_board_binding_replaces_its_default() {
        let mut config = Config::default();
        config.keys.create_task = Some(BindingConfig::one("n"));
        let (mut app, _commands) = app_with_lists_and_config(&["a"], config);
        app.sync = SyncState::Ready;

        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Idle));

        app.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Editing(_)));
    }

    #[test]
    fn empty_configured_binding_disables_the_action() {
        let mut config = Config::default();
        config.keys.delete_task = Some(BindingConfig::empty());
        let (mut app, _commands) = app_with_lists_and_config(&["a"], config);
        app.sync = SyncState::Ready;

        app.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Idle));
    }

    #[test]
    fn configured_modified_chord_dispatches_directly() {
        let mut config = Config::default();
        config.keys.create_task = Some(BindingConfig::one("ctrl+b"));
        let (mut app, _commands) = app_with_lists_and_config(&["a"], config);
        app.sync = SyncState::Ready;

        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Idle));
        app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert!(matches!(app.interaction, Interaction::Editing(_)));
    }

    #[test]
    fn picker_and_editor_actions_use_their_contextual_bindings() {
        let mut config = Config::default();
        config.keys.close_picker = Some(BindingConfig::one("x"));
        config.keys.save_edit = Some(BindingConfig::one("ctrl+s"));
        let (mut app, _commands) = app_with_lists_and_config(&["a"], config);

        app.interaction = Interaction::Move(Picker::new(vec![PickerRow::new(
            "Inbox".into(),
            None,
            "l1".into(),
        )]));
        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Move(_)));
        app.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Idle));

        app.sync = SyncState::Ready;
        app.edit_width = 20;
        app.interaction = Interaction::Editing(new_edit("a", None, "draft"));
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Editing(_)));
        app.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(matches!(app.interaction, Interaction::Idle));
    }

    #[test]
    fn escape_never_quits_from_the_main_board() {
        let (mut app, _commands) = app_with_lists(&["a"]);

        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.should_exit());

        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.should_exit());
    }

    #[test]
    fn question_mark_works_with_both_terminal_key_reports() {
        for event in [
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::SHIFT),
        ] {
            let (mut app, _commands) = app_with_lists(&["a"]);
            app.on_key(event);
            assert!(matches!(app.interaction, Interaction::Help(_)));

            app.on_key(event);
            assert!(matches!(app.interaction, Interaction::Idle));
        }
    }

    #[test]
    fn keyboard_overlay_uses_active_bindings_and_omits_disabled_actions() {
        let mut config = Config::default();
        config.keys.help = Some(BindingConfig::one("f1"));
        config.keys.create_task = Some(BindingConfig::one("n"));
        config.keys.delete_task = Some(BindingConfig::empty());
        let (mut app, _commands) = app_with_lists_and_config(&["a"], config);

        app.on_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        let Interaction::Help(picker) = &app.interaction else {
            panic!("expected help");
        };
        assert!(picker
            .rows()
            .any(|row| { row.title == "n" && row.subtitle.as_deref() == Some("new task") }));
        assert!(!picker
            .rows()
            .any(|row| { row.subtitle.as_deref() == Some("delete task") }));
    }
}
