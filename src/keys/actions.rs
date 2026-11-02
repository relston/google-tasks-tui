//! The action catalog. Each action is declared once, in the table below:
//! its `[keys]` name, the contexts it works in, its default bindings, and
//! its help text. Everything else (the enum, `ALL`, lookups, and the
//! documented default config) is generated from that one line.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BindingContext {
    Board,
    Picker,
    Filter,
    Editor,
    DateEdit,
    Confirm,
}

impl BindingContext {
    /// This context's section heading in the default config.
    pub fn heading(self) -> &'static str {
        match self {
            Self::Board => "Board",
            Self::Picker => "Picker overlays",
            Self::Filter => "Picker filter input",
            Self::Editor => "Task editor",
            Self::DateEdit => "Due date editor",
            Self::Confirm => "Confirmation",
        }
    }
}

macro_rules! actions {
    ($($action:ident = $name:literal, [$($context:ident),+], [$($key:literal),*], $help:literal;)+) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Action {
            $($action,)+
        }

        impl Action {
            pub const ALL: &'static [Self] = &[$(Self::$action,)+];

            /// The action's name under `[keys]` in `config.toml`.
            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$action => $name,)+
                }
            }

            pub fn from_name(name: &str) -> Option<Self> {
                match name {
                    $($name => Some(Self::$action),)+
                    _ => None,
                }
            }

            /// Help text for the Keyboard overlay.
            pub fn description(self) -> &'static str {
                match self {
                    $(Self::$action => $help,)+
                }
            }

            /// Where the action works. The first context places it in the
            /// default config.
            pub fn contexts(self) -> &'static [BindingContext] {
                match self {
                    $(Self::$action => &[$(BindingContext::$context),+],)+
                }
            }

            pub fn defaults(self) -> &'static [&'static str] {
                match self {
                    $(Self::$action => &[$($key),*],)+
                }
            }
        }
    };
}

actions! {
    Quit              = "quit",                [Board],         ["q"],                                  "quit";
    CloseShelf        = "close_shelf",         [Board],         ["esc"],                                "close details";
    PreviousList      = "previous_list",       [Board],         ["h", "left"],                          "previous list";
    NextList          = "next_list",           [Board],         ["l", "right"],                         "next list";
    PreviousTask      = "previous_task",       [Board],         ["k", "up"],                            "previous task";
    NextTask          = "next_task",           [Board],         ["j", "down"],                          "next task";
    FirstTask         = "first_task",          [Board],         ["g", "home"],                          "first task";
    LastTask          = "last_task",           [Board],         ["shift+g", "end"],                     "last task";
    ToggleDetails     = "toggle_details",      [Board],         ["enter"],                              "open / close details";
    NextField         = "next_field",          [Board],         ["tab"],                                "next details field";
    PreviousField     = "previous_field",      [Board],         ["shift+tab"],                          "previous details field";
    Edit              = "edit",                [Board],         ["i"],                                  "edit focused field";
    ToggleDone        = "toggle_done",         [Board],         ["space"],                              "tick / untick";
    ToggleCompleted   = "toggle_completed",    [Board],         ["v"],                                  "show / hide completed";
    DeleteTask        = "delete_task",         [Board],         ["x"],                                  "delete task";
    EditDue           = "edit_due",            [Board],         ["d"],                                  "edit due date";
    CreateTask        = "create_task",         [Board],         ["c"],                                  "new task";
    CreateSubtask     = "create_subtask",      [Board],         ["shift+c"],                            "new subtask";
    Refresh           = "refresh",             [Board],         ["r"],                                  "refresh from Google";
    ToggleTransparent = "toggle_transparent",  [Board],         ["t"],                                  "toggle transparent background";
    CopyTask          = "copy_task",           [Board],         ["y"],                                  "copy task as markdown";
    BrowseLinks       = "browse_links",        [Board],         ["b"],                                  "browse links in notes";
    MoveTask          = "move_task",           [Board],         ["m"],                                  "move to another list";
    ReorderUp         = "reorder_up",          [Board],         ["alt+k", "alt+up"],                    "move task up one place";
    ReorderDown       = "reorder_down",        [Board],         ["alt+j", "alt+down"],                  "move task down one place";
    UnparentTask      = "unparent_task",       [Board],         ["shift+comma"],                        "unparent subtask";
    ChooseParent      = "choose_parent",       [Board],         ["shift+period"],                       "choose parent";
    Help              = "help",                [Board, Picker], ["shift+slash"],                        "keyboard commands";
    ClosePicker       = "close_picker",        [Picker],        ["esc", "q"],                           "close current overlay";
    StartFilter       = "start_filter",        [Picker],        ["slash"],                              "filter current overlay";
    PickerNext        = "picker_next",         [Picker],        ["j", "down"],                          "next overlay item";
    PickerPrevious    = "picker_previous",     [Picker],        ["k", "up"],                            "previous overlay item";
    PickerAccept      = "picker_accept",       [Picker],        ["enter"],                              "choose overlay item";
    PickerAcceptAll   = "picker_accept_all",   [Picker],        ["shift+enter"],                        "choose all matching items";
    StopFilter        = "stop_filter",         [Filter],        ["esc"],                                "stop filtering";
    FilterNext        = "filter_next",         [Filter],        ["down"],                               "next filtered item";
    FilterPrevious    = "filter_previous",     [Filter],        ["up"],                                 "previous filtered item";
    FilterAccept      = "filter_accept",       [Filter],        ["enter"],                              "choose filtered item";
    FilterAcceptAll   = "filter_accept_all",   [Filter],        ["shift+enter"],                        "choose all filtered items";
    FilterBackspace   = "filter_backspace",    [Filter],        ["backspace"],                          "delete filter character";
    CancelEdit        = "cancel_edit",         [Editor, DateEdit], ["esc"],                             "cancel edit";
    SaveEdit          = "save_edit",           [Editor, DateEdit], ["enter"],                           "save edit";
    InsertNewline     = "insert_newline",      [Editor],        ["shift+enter", "alt+enter", "ctrl+j"], "insert notes newline";
    DeleteWord        = "delete_word",         [Editor],        ["ctrl+w"],                             "delete previous word";
    ClearToLineStart  = "clear_to_line_start", [Editor],        ["ctrl+u"],                             "clear to line start";
    CursorLineStart   = "cursor_line_start",   [Editor],        ["ctrl+a", "home"],                     "move to line start";
    CursorLineEnd     = "cursor_line_end",     [Editor],        ["ctrl+e", "end"],                      "move to line end";
    CursorLeft        = "cursor_left",         [Editor],        ["left"],                               "move cursor left";
    CursorRight       = "cursor_right",        [Editor],        ["right"],                              "move cursor right";
    CursorUp          = "cursor_up",           [Editor],        ["up"],                                 "move cursor up";
    CursorDown        = "cursor_down",         [Editor],        ["down"],                               "move cursor down";
    EditorBackspace   = "editor_backspace",    [Editor],        ["backspace"],                          "delete previous character";
    EditorDelete      = "editor_delete",       [Editor],        ["delete"],                             "delete next character";
    ConfirmDelete     = "confirm_delete",      [Confirm],       ["y"],                                  "confirm deletion";
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{parse_binding, parse_key_combo, Keymap};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn every_action_has_a_unique_name_help_text_and_defaults() {
        let mut names = std::collections::HashSet::new();
        for action in Action::ALL {
            assert!(names.insert(action.name()), "duplicate {}", action.name());
            assert_eq!(Action::from_name(action.name()), Some(*action));
            assert!(!action.description().is_empty(), "{action:?}");
            assert!(!action.contexts().is_empty(), "{action:?}");
            assert!(!action.defaults().is_empty(), "{action:?}");
        }
        assert_eq!(Action::from_name("nonsense"), None);
    }

    #[test]
    fn default_catalog_preserves_board_picker_filter_editor_and_confirm_keys() {
        let (keymap, diagnostics) = Keymap::new(&[]);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let cases = [
            (BindingContext::Board, "h", Action::PreviousList),
            (BindingContext::Board, "left", Action::PreviousList),
            (BindingContext::Board, "v", Action::ToggleCompleted),
            (BindingContext::Board, "shift+c", Action::CreateSubtask),
            (BindingContext::Board, "shift+comma", Action::UnparentTask),
            (BindingContext::Board, "shift+slash", Action::Help),
            (BindingContext::Picker, "q", Action::ClosePicker),
            (BindingContext::Picker, "down", Action::PickerNext),
            (
                BindingContext::Picker,
                "shift+enter",
                Action::PickerAcceptAll,
            ),
            (BindingContext::Filter, "esc", Action::StopFilter),
            (BindingContext::Filter, "backspace", Action::FilterBackspace),
            (BindingContext::Editor, "ctrl+w", Action::DeleteWord),
            (BindingContext::Editor, "alt+enter", Action::InsertNewline),
            (BindingContext::Editor, "home", Action::CursorLineStart),
            (BindingContext::DateEdit, "enter", Action::SaveEdit),
            (BindingContext::DateEdit, "esc", Action::CancelEdit),
            (BindingContext::Board, "d", Action::EditDue),
            (BindingContext::Board, "x", Action::DeleteTask),
            (BindingContext::Confirm, "y", Action::ConfirmDelete),
        ];

        for (context, raw, expected) in cases {
            let combo = parse_key_combo(raw).unwrap();
            assert_eq!(
                keymap.resolve(context, &event(combo.0, combo.1)),
                Some(expected),
                "{context:?} {raw}"
            );
        }
    }

    #[test]
    fn every_default_binding_resolves_to_its_action_in_every_context() {
        let (keymap, diagnostics) = Keymap::new(&[]);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");

        for action in Action::ALL {
            for context in action.contexts() {
                for raw in action.defaults() {
                    let binding = parse_binding(raw).unwrap();
                    assert_eq!(
                        keymap.resolve(*context, &event(binding.combo.0, binding.combo.1)),
                        Some(*action),
                        "{} = {raw}",
                        action.name()
                    );
                }
            }
        }
    }

    #[test]
    fn labels_deduplicate_actions_registered_in_multiple_contexts() {
        let (keymap, diagnostics) = Keymap::new(&[]);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(keymap.labels_for(Action::Help), ["shift+/"]);
    }
}
