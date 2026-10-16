//! The action catalog: every command, its config name, help text, and
//! default bindings per context.

use super::keymap::{BindingConfig, BindingSpec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BindingContext {
    Board,
    Picker,
    Filter,
    Editor,
    Confirm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    CloseShelf,
    PreviousList,
    NextList,
    PreviousTask,
    NextTask,
    FirstTask,
    LastTask,
    ToggleDetails,
    NextField,
    PreviousField,
    Edit,
    ToggleDone,
    ToggleCompleted,
    DeleteTask,
    CreateTask,
    CreateSubtask,
    Refresh,
    ToggleTransparent,
    CopyTask,
    BrowseLinks,
    MoveTask,
    ReorderUp,
    ReorderDown,
    UnparentTask,
    ChooseParent,
    Help,
    ClosePicker,
    StartFilter,
    PickerNext,
    PickerPrevious,
    PickerAccept,
    PickerAcceptAll,
    StopFilter,
    FilterNext,
    FilterPrevious,
    FilterAccept,
    FilterAcceptAll,
    FilterBackspace,
    CancelEdit,
    SaveEdit,
    InsertNewline,
    DeleteWord,
    ClearToLineStart,
    CursorLineStart,
    CursorLineEnd,
    CursorLeft,
    CursorRight,
    CursorUp,
    CursorDown,
    EditorBackspace,
    EditorDelete,
    ConfirmDelete,
}

impl Action {
    pub const ALL: &'static [Self] = &[
        Self::Quit,
        Self::CloseShelf,
        Self::PreviousList,
        Self::NextList,
        Self::PreviousTask,
        Self::NextTask,
        Self::FirstTask,
        Self::LastTask,
        Self::ToggleDetails,
        Self::NextField,
        Self::PreviousField,
        Self::Edit,
        Self::ToggleDone,
        Self::ToggleCompleted,
        Self::DeleteTask,
        Self::CreateTask,
        Self::CreateSubtask,
        Self::Refresh,
        Self::ToggleTransparent,
        Self::CopyTask,
        Self::BrowseLinks,
        Self::MoveTask,
        Self::ReorderUp,
        Self::ReorderDown,
        Self::UnparentTask,
        Self::ChooseParent,
        Self::Help,
        Self::ClosePicker,
        Self::StartFilter,
        Self::PickerNext,
        Self::PickerPrevious,
        Self::PickerAccept,
        Self::PickerAcceptAll,
        Self::StopFilter,
        Self::FilterNext,
        Self::FilterPrevious,
        Self::FilterAccept,
        Self::FilterAcceptAll,
        Self::FilterBackspace,
        Self::CancelEdit,
        Self::SaveEdit,
        Self::InsertNewline,
        Self::DeleteWord,
        Self::ClearToLineStart,
        Self::CursorLineStart,
        Self::CursorLineEnd,
        Self::CursorLeft,
        Self::CursorRight,
        Self::CursorUp,
        Self::CursorDown,
        Self::EditorBackspace,
        Self::EditorDelete,
        Self::ConfirmDelete,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Quit => "quit",
            Self::CloseShelf => "close_shelf",
            Self::PreviousList => "previous_list",
            Self::NextList => "next_list",
            Self::PreviousTask => "previous_task",
            Self::NextTask => "next_task",
            Self::FirstTask => "first_task",
            Self::LastTask => "last_task",
            Self::ToggleDetails => "toggle_details",
            Self::NextField => "next_field",
            Self::PreviousField => "previous_field",
            Self::Edit => "edit",
            Self::ToggleDone => "toggle_done",
            Self::ToggleCompleted => "toggle_completed",
            Self::DeleteTask => "delete_task",
            Self::CreateTask => "create_task",
            Self::CreateSubtask => "create_subtask",
            Self::Refresh => "refresh",
            Self::ToggleTransparent => "toggle_transparent",
            Self::CopyTask => "copy_task",
            Self::BrowseLinks => "browse_links",
            Self::MoveTask => "move_task",
            Self::ReorderUp => "reorder_up",
            Self::ReorderDown => "reorder_down",
            Self::UnparentTask => "unparent_task",
            Self::ChooseParent => "choose_parent",
            Self::Help => "help",
            Self::ClosePicker => "close_picker",
            Self::StartFilter => "start_filter",
            Self::PickerNext => "picker_next",
            Self::PickerPrevious => "picker_previous",
            Self::PickerAccept => "picker_accept",
            Self::PickerAcceptAll => "picker_accept_all",
            Self::StopFilter => "stop_filter",
            Self::FilterNext => "filter_next",
            Self::FilterPrevious => "filter_previous",
            Self::FilterAccept => "filter_accept",
            Self::FilterAcceptAll => "filter_accept_all",
            Self::FilterBackspace => "filter_backspace",
            Self::CancelEdit => "cancel_edit",
            Self::SaveEdit => "save_edit",
            Self::InsertNewline => "insert_newline",
            Self::DeleteWord => "delete_word",
            Self::ClearToLineStart => "clear_to_line_start",
            Self::CursorLineStart => "cursor_line_start",
            Self::CursorLineEnd => "cursor_line_end",
            Self::CursorLeft => "cursor_left",
            Self::CursorRight => "cursor_right",
            Self::CursorUp => "cursor_up",
            Self::CursorDown => "cursor_down",
            Self::EditorBackspace => "editor_backspace",
            Self::EditorDelete => "editor_delete",
            Self::ConfirmDelete => "confirm_delete",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Quit => "quit",
            Self::CloseShelf => "close details",
            Self::PreviousList => "previous list",
            Self::NextList => "next list",
            Self::PreviousTask => "previous task",
            Self::NextTask => "next task",
            Self::FirstTask => "first task",
            Self::LastTask => "last task",
            Self::ToggleDetails => "open / close details",
            Self::NextField => "next details field",
            Self::PreviousField => "previous details field",
            Self::Edit => "edit focused field",
            Self::ToggleDone => "tick / untick",
            Self::ToggleCompleted => "show / hide completed",
            Self::DeleteTask => "delete task",
            Self::CreateTask => "new task",
            Self::CreateSubtask => "new subtask",
            Self::Refresh => "refresh from Google",
            Self::ToggleTransparent => "toggle transparent background",
            Self::CopyTask => "copy task as markdown",
            Self::BrowseLinks => "browse links in notes",
            Self::MoveTask => "move to another list",
            Self::ReorderUp => "move task up one place",
            Self::ReorderDown => "move task down one place",
            Self::UnparentTask => "unparent subtask",
            Self::ChooseParent => "choose parent",
            Self::Help => "keyboard commands",
            Self::ClosePicker => "close current overlay",
            Self::StartFilter => "filter current overlay",
            Self::PickerNext => "next overlay item",
            Self::PickerPrevious => "previous overlay item",
            Self::PickerAccept => "choose overlay item",
            Self::PickerAcceptAll => "choose all matching items",
            Self::StopFilter => "stop filtering",
            Self::FilterNext => "next filtered item",
            Self::FilterPrevious => "previous filtered item",
            Self::FilterAccept => "choose filtered item",
            Self::FilterAcceptAll => "choose all filtered items",
            Self::FilterBackspace => "delete filter character",
            Self::CancelEdit => "cancel edit",
            Self::SaveEdit => "save edit",
            Self::InsertNewline => "insert notes newline",
            Self::DeleteWord => "delete previous word",
            Self::ClearToLineStart => "clear to line start",
            Self::CursorLineStart => "move to line start",
            Self::CursorLineEnd => "move to line end",
            Self::CursorLeft => "move cursor left",
            Self::CursorRight => "move cursor right",
            Self::CursorUp => "move cursor up",
            Self::CursorDown => "move cursor down",
            Self::EditorBackspace => "delete previous character",
            Self::EditorDelete => "delete next character",
            Self::ConfirmDelete => "confirm deletion",
        }
    }
}

pub fn default_binding_specs() -> Vec<BindingSpec<Action>> {
    use Action::*;
    use BindingContext::*;

    fn spec(context: BindingContext, action: Action, bindings: &[&str]) -> BindingSpec<Action> {
        BindingSpec::default(
            context,
            action,
            action.name(),
            BindingConfig::many(bindings.iter().copied()),
        )
    }

    vec![
        spec(Board, Quit, &["q"]),
        spec(Board, CloseShelf, &["esc"]),
        spec(Board, PreviousList, &["h", "left"]),
        spec(Board, NextList, &["l", "right"]),
        spec(Board, PreviousTask, &["k", "up"]),
        spec(Board, NextTask, &["j", "down"]),
        spec(Board, FirstTask, &["g", "home"]),
        spec(Board, LastTask, &["shift+g", "end"]),
        spec(Board, ToggleDetails, &["enter"]),
        spec(Board, NextField, &["tab"]),
        spec(Board, PreviousField, &["shift+tab"]),
        spec(Board, Edit, &["i"]),
        spec(Board, ToggleDone, &["space"]),
        spec(Board, ToggleCompleted, &["v"]),
        spec(Board, DeleteTask, &["d"]),
        spec(Board, CreateTask, &["c"]),
        spec(Board, CreateSubtask, &["shift+c"]),
        spec(Board, Refresh, &["r"]),
        spec(Board, ToggleTransparent, &["t"]),
        spec(Board, CopyTask, &["y"]),
        spec(Board, BrowseLinks, &["b"]),
        spec(Board, MoveTask, &["m"]),
        spec(Board, ReorderUp, &["alt+k", "alt+up"]),
        spec(Board, ReorderDown, &["alt+j", "alt+down"]),
        spec(Board, UnparentTask, &["shift+comma"]),
        spec(Board, ChooseParent, &["shift+period"]),
        spec(Board, Help, &["shift+slash"]),
        spec(Picker, Help, &["shift+slash"]),
        spec(Picker, ClosePicker, &["esc", "q"]),
        spec(Picker, StartFilter, &["slash"]),
        spec(Picker, PickerNext, &["j", "down"]),
        spec(Picker, PickerPrevious, &["k", "up"]),
        spec(Picker, PickerAccept, &["enter"]),
        spec(Picker, PickerAcceptAll, &["shift+enter"]),
        spec(Filter, StopFilter, &["esc"]),
        spec(Filter, FilterNext, &["down"]),
        spec(Filter, FilterPrevious, &["up"]),
        spec(Filter, FilterAccept, &["enter"]),
        spec(Filter, FilterAcceptAll, &["shift+enter"]),
        spec(Filter, FilterBackspace, &["backspace"]),
        spec(Editor, CancelEdit, &["esc"]),
        spec(Editor, SaveEdit, &["enter"]),
        spec(
            Editor,
            InsertNewline,
            &["shift+enter", "alt+enter", "ctrl+j"],
        ),
        spec(Editor, DeleteWord, &["ctrl+w"]),
        spec(Editor, ClearToLineStart, &["ctrl+u"]),
        spec(Editor, CursorLineStart, &["ctrl+a", "home"]),
        spec(Editor, CursorLineEnd, &["ctrl+e", "end"]),
        spec(Editor, CursorLeft, &["left"]),
        spec(Editor, CursorRight, &["right"]),
        spec(Editor, CursorUp, &["up"]),
        spec(Editor, CursorDown, &["down"]),
        spec(Editor, EditorBackspace, &["backspace"]),
        spec(Editor, EditorDelete, &["delete"]),
        spec(Confirm, ConfirmDelete, &["y"]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{default_keymap, parse_binding, parse_key_combo};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn action_catalog_names_every_command_and_supplies_defaults() {
        assert_eq!(Action::ALL.len(), 53);
        let mut names = std::collections::HashSet::new();
        let specs = default_binding_specs();

        for action in Action::ALL {
            assert!(names.insert(action.name()), "duplicate {}", action.name());
            assert!(!action.description().is_empty(), "{action:?}");
            assert!(
                specs.iter().any(|spec| spec.action == *action),
                "missing default for {action:?}"
            );
        }
    }

    #[test]
    fn default_catalog_preserves_board_picker_filter_editor_and_confirm_keys() {
        let (keymap, diagnostics) = default_keymap();
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
    fn every_default_binding_resolves_to_its_declared_action_and_context() {
        let specs = default_binding_specs();
        let (keymap, diagnostics) = default_keymap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");

        for spec in specs {
            for raw in spec.config.values() {
                let binding = parse_binding(raw).unwrap();
                assert_eq!(
                    keymap.resolve(spec.context, &event(binding.combo.0, binding.combo.1)),
                    Some(spec.action),
                    "{} = {raw}",
                    spec.name
                );
            }
        }
    }

    #[test]
    fn labels_deduplicate_actions_registered_in_multiple_contexts() {
        let (keymap, diagnostics) = default_keymap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(keymap.labels_for(Action::Help), ["shift+/"]);
    }
}
