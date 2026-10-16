//! Configurable keybinding parsing, validation, and event matching.
//!
//! This module deliberately has no knowledge of TUI behavior. The action
//! catalog and dispatch layer are added separately so this boundary can be
//! tested without constructing an application.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::fmt;

pub type KeyCombo = (KeyCode, KeyModifiers);

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum BindingConfig {
    One(String),
    Many(Vec<String>),
}

impl Default for BindingConfig {
    fn default() -> Self {
        Self::Many(Vec::new())
    }
}

impl BindingConfig {
    pub fn one(value: impl Into<String>) -> Self {
        Self::One(value.into())
    }

    pub fn many(values: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::Many(values.into_iter().map(Into::into).collect())
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn values(&self) -> Vec<&str> {
        match self {
            Self::One(value) if value.trim().is_empty() => Vec::new(),
            Self::One(value) => vec![value],
            Self::Many(values) => values.iter().map(String::as_str).collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedBinding {
    pub combo: KeyCombo,
    pub label: String,
}

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
    MoveTaskUp,
    MoveTaskDown,
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
        Self::MoveTaskUp,
        Self::MoveTaskDown,
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
            Self::MoveTaskUp => "move_task_up",
            Self::MoveTaskDown => "move_task_down",
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
            Self::MoveTaskUp => "move task up",
            Self::MoveTaskDown => "move task down",
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingSource {
    Default,
    User,
}

#[derive(Clone, Debug)]
pub struct BindingSpec<A> {
    pub context: BindingContext,
    pub action: A,
    pub name: &'static str,
    pub config: BindingConfig,
    pub source: BindingSource,
}

impl<A> BindingSpec<A> {
    pub fn default(
        context: BindingContext,
        action: A,
        name: &'static str,
        config: BindingConfig,
    ) -> Self {
        Self {
            context,
            action,
            name,
            config,
            source: BindingSource::Default,
        }
    }

    pub fn user(
        context: BindingContext,
        action: A,
        name: &'static str,
        config: BindingConfig,
    ) -> Self {
        Self {
            context,
            action,
            name,
            config,
            source: BindingSource::User,
        }
    }
}

#[derive(Clone, Debug)]
struct KeymapEntry<A> {
    context: BindingContext,
    action: A,
    action_name: &'static str,
    source: BindingSource,
    binding: ResolvedBinding,
}

#[derive(Clone, Debug)]
pub struct Keymap<A> {
    entries: Vec<KeymapEntry<A>>,
}

impl<A: Clone + PartialEq> Keymap<A> {
    /// Resolve user entries first. This gives an explicit override priority
    /// over a shipped default even when that default appeared first in the
    /// source catalog.
    pub fn build(specs: &[BindingSpec<A>]) -> (Self, Vec<String>) {
        let mut entries: Vec<KeymapEntry<A>> = Vec::new();
        let mut diagnostics = Vec::new();

        for source in [BindingSource::User, BindingSource::Default] {
            for spec in specs.iter().filter(|spec| spec.source == source) {
                for raw in spec.config.values() {
                    let raw = raw.trim();
                    if raw.is_empty() {
                        continue;
                    }
                    let binding = match parse_binding(raw) {
                        Ok(binding) => binding,
                        Err(error) => {
                            diagnostics.push(format!(
                                "invalid keybinding: {} = {raw:?}: {error}; disabling binding",
                                spec.name
                            ));
                            continue;
                        }
                    };

                    if matches!(
                        spec.context,
                        BindingContext::Editor | BindingContext::Filter
                    ) && matches!(
                        binding.combo,
                        (KeyCode::Char(_), modifiers) if modifiers.is_empty()
                    ) {
                        diagnostics.push(format!(
                            "unsafe keybinding: {} = {raw:?} would intercept typing; disabling binding",
                            spec.name
                        ));
                        continue;
                    }

                    let conflict = entries.iter().find(|entry| {
                        entry.context == spec.context
                            && normalize_key_combo(entry.binding.combo)
                                == normalize_key_combo(binding.combo)
                    });
                    if let Some(conflict) = conflict {
                        if source == BindingSource::Default
                            && conflict.source == BindingSource::User
                        {
                            continue;
                        }
                        diagnostics.push(format!(
                            "{}: kept {}, disabled {}",
                            binding.label, conflict.action_name, spec.name
                        ));
                        continue;
                    }

                    entries.push(KeymapEntry {
                        context: spec.context,
                        action: spec.action.clone(),
                        action_name: spec.name,
                        source,
                        binding,
                    });
                }
            }
        }

        diagnostics.dedup();
        (Self { entries }, diagnostics)
    }

    pub fn resolve(&self, context: BindingContext, key: &KeyEvent) -> KeyResolution<A> {
        self.matching(context, key)
            .map(|entry| KeyResolution::Action(entry.action.clone()))
            .unwrap_or(KeyResolution::None)
    }

    pub fn bindings_for(&self, action: A) -> Vec<&ResolvedBinding> {
        self.entries
            .iter()
            .filter(|entry| entry.action == action)
            .map(|entry| &entry.binding)
            .collect()
    }

    pub fn labels_for(&self, action: A) -> Vec<String> {
        let mut labels = Vec::new();
        for binding in self.bindings_for(action) {
            if !labels.contains(&binding.label) {
                labels.push(binding.label.clone());
            }
        }
        labels
    }

    fn matching(&self, context: BindingContext, key: &KeyEvent) -> Option<&KeymapEntry<A>> {
        self.entries.iter().find(|entry| {
            entry.context == context && key_event_matches_combo(key, entry.binding.combo)
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyResolution<A> {
    None,
    Action(A),
}

pub fn default_keymap() -> (Keymap<Action>, Vec<String>) {
    Keymap::build(&default_binding_specs())
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
        spec(Board, MoveTaskUp, &["alt+k", "alt+up"]),
        spec(Board, MoveTaskDown, &["alt+j", "alt+down"]),
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseKeyError(String);

impl fmt::Display for ParseKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ParseKeyError {}

pub fn parse_binding(raw: &str) -> Result<ResolvedBinding, ParseKeyError> {
    let combo = parse_key_combo(raw.trim())?;
    Ok(ResolvedBinding {
        combo,
        label: format_key_combo(combo),
    })
}

pub fn parse_key_combo(raw: &str) -> Result<KeyCombo, ParseKeyError> {
    let mut modifiers = KeyModifiers::NONE;
    let mut key = None;

    for part in raw.split('+') {
        let part = part.trim();
        if part.is_empty() {
            return Err(ParseKeyError("empty key component".into()));
        }
        if let Some(modifier) = parse_modifier(part) {
            modifiers |= modifier;
        } else if key.replace(part).is_some() {
            return Err(ParseKeyError("more than one key".into()));
        }
    }

    let key = key.ok_or_else(|| ParseKeyError("missing key".into()))?;
    let lower = key.to_lowercase();
    let mut code = match lower.as_str() {
        "space" => KeyCode::Char(' '),
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "backspace" | "bs" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "page_up" | "page-up" => KeyCode::PageUp,
        "pagedown" | "page_down" | "page-down" => KeyCode::PageDown,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "minus" => KeyCode::Char('-'),
        "equals" | "equal" => KeyCode::Char('='),
        "comma" => KeyCode::Char(','),
        "period" | "dot" => KeyCode::Char('.'),
        "slash" => KeyCode::Char('/'),
        "backslash" => KeyCode::Char('\\'),
        "quote" => KeyCode::Char('\''),
        "double_quote" | "double-quote" => KeyCode::Char('"'),
        "semicolon" => KeyCode::Char(';'),
        "colon" => KeyCode::Char(':'),
        "percent" => KeyCode::Char('%'),
        "ampersand" => KeyCode::Char('&'),
        "backtick" => KeyCode::Char('`'),
        "plus" => KeyCode::Char('+'),
        _ => {
            if let Some(function) = lower.strip_prefix('f') {
                let number = function
                    .parse::<u8>()
                    .ok()
                    .filter(|number| (1..=24).contains(number))
                    .ok_or_else(|| ParseKeyError(format!("unknown key {key:?}")))?;
                KeyCode::F(number)
            } else {
                let mut chars = key.chars();
                let character = chars
                    .next()
                    .filter(|_| chars.next().is_none())
                    .ok_or_else(|| ParseKeyError(format!("unknown key {key:?}")))?;
                KeyCode::Char(character)
            }
        }
    };

    if let KeyCode::Char(character) = code {
        if character.is_ascii_uppercase() {
            code = KeyCode::Char(character.to_ascii_lowercase());
            modifiers |= KeyModifiers::SHIFT;
        } else if let Some(base) = unshifted_symbol(character) {
            code = KeyCode::Char(base);
            modifiers |= KeyModifiers::SHIFT;
        }
    }

    if code == KeyCode::Tab && modifiers.contains(KeyModifiers::SHIFT) {
        code = KeyCode::BackTab;
        modifiers.remove(KeyModifiers::SHIFT);
    }

    Ok(normalize_key_combo((code, modifiers)))
}

fn parse_modifier(raw: &str) -> Option<KeyModifiers> {
    match raw.to_lowercase().as_str() {
        "ctrl" | "control" => Some(KeyModifiers::CONTROL),
        "shift" => Some(KeyModifiers::SHIFT),
        "alt" | "option" | "meta" => Some(KeyModifiers::ALT),
        "cmd" | "command" | "super" => Some(KeyModifiers::SUPER),
        "hyper" => Some(KeyModifiers::HYPER),
        _ => None,
    }
}

pub fn normalize_key_combo((mut code, mut modifiers): KeyCombo) -> KeyCombo {
    if code == KeyCode::Tab && modifiers.contains(KeyModifiers::SHIFT) {
        code = KeyCode::BackTab;
        modifiers.remove(KeyModifiers::SHIFT);
    } else if code == KeyCode::BackTab {
        modifiers.remove(KeyModifiers::SHIFT);
    }
    (code, modifiers)
}

pub fn key_event_matches_combo(event: &KeyEvent, expected: KeyCombo) -> bool {
    normalize_reported_combo((event.code, event.modifiers)) == normalize_reported_combo(expected)
}

fn normalize_reported_combo((mut code, mut modifiers): KeyCombo) -> KeyCombo {
    (code, modifiers) = normalize_key_combo((code, modifiers));
    if let KeyCode::Char(character) = code {
        if character.is_ascii_uppercase() {
            code = KeyCode::Char(character.to_ascii_lowercase());
            modifiers |= KeyModifiers::SHIFT;
        } else if let Some(base) = unshifted_symbol(character) {
            code = KeyCode::Char(base);
            modifiers |= KeyModifiers::SHIFT;
        }
    }
    normalize_key_combo((code, modifiers))
}

fn unshifted_symbol(symbol: char) -> Option<char> {
    const SHIFTED: &[(char, char)] = &[
        ('!', '1'),
        ('@', '2'),
        ('#', '3'),
        ('$', '4'),
        ('%', '5'),
        ('^', '6'),
        ('&', '7'),
        ('*', '8'),
        ('(', '9'),
        (')', '0'),
        ('_', '-'),
        ('+', '='),
        ('{', '['),
        ('}', ']'),
        ('|', '\\'),
        (':', ';'),
        ('"', '\''),
        ('<', ','),
        ('>', '.'),
        ('?', '/'),
        ('~', '`'),
    ];
    SHIFTED
        .iter()
        .find_map(|(shifted, base)| (*shifted == symbol).then_some(*base))
}

pub fn format_key_combo(combo: KeyCombo) -> String {
    let (code, mut modifiers) = normalize_key_combo(combo);
    if code == KeyCode::BackTab {
        modifiers |= KeyModifiers::SHIFT;
    }
    let mut parts = Vec::new();
    if modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("ctrl".to_string());
    }
    if modifiers.contains(KeyModifiers::ALT) {
        parts.push("alt".to_string());
    }
    if modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("shift".to_string());
    }
    if modifiers.contains(KeyModifiers::SUPER) {
        parts.push(if cfg!(target_os = "macos") {
            "cmd".to_string()
        } else {
            "super".to_string()
        });
    }
    if modifiers.contains(KeyModifiers::HYPER) {
        parts.push("hyper".to_string());
    }
    if modifiers.contains(KeyModifiers::META) {
        parts.push("meta".to_string());
    }

    if code == KeyCode::BackTab {
        parts.push("tab".to_string());
    } else {
        parts.push(match code {
            KeyCode::Char(' ') => "space".into(),
            KeyCode::Char(character) => character.to_string(),
            KeyCode::Enter => "enter".into(),
            KeyCode::Esc => "esc".into(),
            KeyCode::Tab => "tab".into(),
            KeyCode::Backspace => "backspace".into(),
            KeyCode::Delete => "delete".into(),
            KeyCode::Insert => "insert".into(),
            KeyCode::Home => "home".into(),
            KeyCode::End => "end".into(),
            KeyCode::PageUp => "pageup".into(),
            KeyCode::PageDown => "pagedown".into(),
            KeyCode::Left => "left".into(),
            KeyCode::Right => "right".into(),
            KeyCode::Up => "up".into(),
            KeyCode::Down => "down".into(),
            KeyCode::F(number) => format!("f{number}"),
            other => format!("{other:?}").to_lowercase(),
        });
    }
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use serde::Deserialize;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum TestAction {
        New,
        Help,
        Pick,
    }

    #[derive(Deserialize)]
    struct BindingFixture {
        binding: BindingConfig,
    }

    fn event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn binding_config_accepts_one_many_and_empty_values() {
        let one: BindingFixture = toml::from_str(r#"binding = "ctrl+n""#).unwrap();
        assert_eq!(one.binding.values(), ["ctrl+n"]);

        let many: BindingFixture = toml::from_str(r#"binding = ["j", "down", ""]"#).unwrap();
        assert_eq!(many.binding.values(), ["j", "down", ""]);

        assert!(BindingConfig::empty().values().is_empty());
    }

    #[test]
    fn parses_modifiers_special_keys_unicode_and_named_punctuation() {
        assert_eq!(
            parse_key_combo("ctrl+alt+a").unwrap(),
            (
                KeyCode::Char('a'),
                KeyModifiers::CONTROL | KeyModifiers::ALT
            )
        );
        assert_eq!(
            parse_key_combo("C").unwrap(),
            (KeyCode::Char('c'), KeyModifiers::SHIFT)
        );
        assert_eq!(
            parse_key_combo("shift+tab").unwrap(),
            (KeyCode::BackTab, KeyModifiers::NONE)
        );
        assert_eq!(
            parse_key_combo("f12").unwrap(),
            (KeyCode::F(12), KeyModifiers::NONE)
        );
        assert_eq!(
            parse_key_combo("enter").unwrap(),
            (KeyCode::Enter, KeyModifiers::NONE)
        );
        assert_eq!(
            parse_key_combo("ö").unwrap(),
            (KeyCode::Char('ö'), KeyModifiers::NONE)
        );
        assert_eq!(
            parse_key_combo("shift+comma").unwrap(),
            (KeyCode::Char(','), KeyModifiers::SHIFT)
        );
        assert_eq!(
            parse_key_combo("plus").unwrap(),
            (KeyCode::Char('='), KeyModifiers::SHIFT)
        );
    }

    #[test]
    fn rejects_missing_multiple_and_unknown_keys() {
        for raw in ["", "ctrl", "ctrl+a+b", "f0", "f25", "not-a-key"] {
            assert!(parse_key_combo(raw).is_err(), "{raw:?} should be invalid");
        }
        assert!(parse_binding("prefix+c").is_err());
    }

    #[test]
    fn shifted_bindings_match_terminal_reporting_variants() {
        let question = parse_key_combo("shift+slash").unwrap();
        for key in [
            event(KeyCode::Char('/'), KeyModifiers::SHIFT),
            event(KeyCode::Char('?'), KeyModifiers::NONE),
            event(KeyCode::Char('?'), KeyModifiers::SHIFT),
        ] {
            assert!(key_event_matches_combo(&key, question), "{key:?}");
        }

        let less_than = parse_key_combo("shift+comma").unwrap();
        for key in [
            event(KeyCode::Char(','), KeyModifiers::SHIFT),
            event(KeyCode::Char('<'), KeyModifiers::NONE),
            event(KeyCode::Char('<'), KeyModifiers::SHIFT),
        ] {
            assert!(key_event_matches_combo(&key, less_than), "{key:?}");
        }

        let upper = parse_key_combo("shift+g").unwrap();
        assert!(key_event_matches_combo(
            &event(KeyCode::Char('G'), KeyModifiers::NONE),
            upper
        ));
        assert!(key_event_matches_combo(
            &event(KeyCode::Char('g'), KeyModifiers::SHIFT),
            upper
        ));
        assert!(!key_event_matches_combo(
            &event(KeyCode::Char('g'), KeyModifiers::CONTROL),
            upper
        ));
    }

    #[test]
    fn formats_normalized_key_combinations() {
        assert_eq!(
            format_key_combo((KeyCode::Char('c'), KeyModifiers::CONTROL)),
            "ctrl+c"
        );
        assert_eq!(
            format_key_combo((KeyCode::BackTab, KeyModifiers::NONE)),
            "shift+tab"
        );
        assert_eq!(
            format_key_combo((KeyCode::Char(' '), KeyModifiers::NONE)),
            "space"
        );
    }

    #[test]
    fn explicit_user_bindings_displace_defaults_in_the_same_context() {
        let specs = [
            BindingSpec::default(
                BindingContext::Board,
                TestAction::New,
                "new",
                BindingConfig::one("c"),
            ),
            BindingSpec::user(
                BindingContext::Board,
                TestAction::Help,
                "help",
                BindingConfig::one("c"),
            ),
            BindingSpec::default(
                BindingContext::Picker,
                TestAction::Pick,
                "pick",
                BindingConfig::one("c"),
            ),
        ];
        let (keymap, diagnostics) = Keymap::build(&specs);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");

        let c = event(KeyCode::Char('c'), KeyModifiers::NONE);
        assert_eq!(
            keymap.resolve(BindingContext::Board, &c),
            KeyResolution::Action(TestAction::Help)
        );
        assert_eq!(
            keymap.resolve(BindingContext::Picker, &c),
            KeyResolution::Action(TestAction::Pick)
        );
        assert!(keymap.bindings_for(TestAction::New).is_empty());
    }

    #[test]
    fn conflicting_user_bindings_keep_the_first_and_report_the_second() {
        let specs = [
            BindingSpec::user(
                BindingContext::Board,
                TestAction::New,
                "new",
                BindingConfig::one("c"),
            ),
            BindingSpec::user(
                BindingContext::Board,
                TestAction::Help,
                "help",
                BindingConfig::one("c"),
            ),
        ];
        let (keymap, diagnostics) = Keymap::build(&specs);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].contains("kept new"));
        assert!(diagnostics[0].contains("disabled help"));

        assert_eq!(
            keymap.resolve(
                BindingContext::Board,
                &event(KeyCode::Char('c'), KeyModifiers::NONE)
            ),
            KeyResolution::Action(TestAction::New)
        );
    }

    #[test]
    fn empty_bindings_disable_an_action_without_diagnostics() {
        let specs = [BindingSpec::user(
            BindingContext::Board,
            TestAction::New,
            "new",
            BindingConfig::empty(),
        )];
        let (keymap, diagnostics) = Keymap::build(&specs);
        assert!(diagnostics.is_empty());
        assert!(keymap.bindings_for(TestAction::New).is_empty());
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
                KeyResolution::Action(expected),
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
                    KeyResolution::Action(spec.action),
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

    #[test]
    fn text_entry_contexts_reject_unmodified_printable_commands() {
        let specs = [
            BindingSpec::user(
                BindingContext::Editor,
                TestAction::New,
                "editor_action",
                BindingConfig::one("x"),
            ),
            BindingSpec::user(
                BindingContext::Filter,
                TestAction::Help,
                "filter_action",
                BindingConfig::one("y"),
            ),
        ];
        let (keymap, diagnostics) = Keymap::build(&specs);
        assert!(keymap.bindings_for(TestAction::New).is_empty());
        assert!(keymap.bindings_for(TestAction::Help).is_empty());
        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics
            .iter()
            .all(|message| message.contains("would intercept typing")));
    }
}
