//! The filterable modal list behind Browse, Move, Select Parent, and the
//! Keyboard overlay.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::keys::Action;

use super::input::typed;

/// One row of a modal list. `value` is what the caller acts on — a URL for
/// Browse, a list id for Move, nothing for the Keyboard table — and `filter`
/// is the lowercased haystack the `/` query matches against.
pub(super) struct PickerRow {
    pub(super) title: String,
    pub(super) subtitle: Option<String>,
    pub(super) value: String,
    filter: String,
}

impl PickerRow {
    pub(super) fn new(title: String, subtitle: Option<String>, value: String) -> Self {
        let mut filter = title.to_lowercase();
        if let Some(subtitle) = &subtitle {
            filter.push(' ');
            filter.push_str(&subtitle.to_lowercase());
        }
        Self {
            title,
            subtitle,
            value,
            filter,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PickerStyle {
    /// Rows to choose from, each a title above its subtitle.
    Choose,
    /// A read-only card: `title` is a key column beside `subtitle`, and
    /// nothing is selectable.
    Reference,
}

/// A modal list with a `/` filter. One of these backs Browse, Move and the
/// Keyboard overlay, so all three share layout, filtering and empty state.
pub(super) struct Picker {
    rows: Vec<PickerRow>,
    /// Indexes into `rows` that survive the current query, in source order.
    shown: Vec<usize>,
    query: String,
    /// `/` was pressed: keys go to the query instead of the list.
    typing: bool,
    style: PickerStyle,
    /// Index into `shown`; `None` when nothing is selectable.
    selected: Option<usize>,
}

impl Picker {
    pub(super) fn new(rows: Vec<PickerRow>) -> Self {
        Self {
            shown: (0..rows.len()).collect(),
            rows,
            query: String::new(),
            typing: false,
            style: PickerStyle::Choose,
            selected: Some(0),
        }
    }

    pub(super) fn reference(rows: Vec<PickerRow>) -> Self {
        Self {
            style: PickerStyle::Reference,
            selected: None,
            ..Self::new(rows)
        }
    }

    pub(super) fn rows(&self) -> impl Iterator<Item = &PickerRow> {
        self.shown.iter().map(|index| &self.rows[*index])
    }

    pub(super) fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    pub(super) fn query(&self) -> &str {
        &self.query
    }

    pub(super) fn typing(&self) -> bool {
        self.typing
    }

    pub(super) fn style(&self) -> PickerStyle {
        self.style
    }

    pub(super) fn selected(&self) -> Option<&PickerRow> {
        self.selected
            .and_then(|index| self.shown.get(index))
            .map(|index| &self.rows[*index])
    }

    /// The selected row's position among the shown rows, for drawing.
    pub(super) fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    /// Every value still passing the filter, so "open all" opens what you
    /// narrowed to rather than everything.
    pub(super) fn values(&self) -> Vec<String> {
        self.rows().map(|row| row.value.clone()).collect()
    }

    pub(super) fn step(&mut self, delta: isize) {
        let Some(last) = self.shown.len().checked_sub(1) else {
            return;
        };
        let current = self.selected.unwrap_or(0) as isize;
        self.selected = Some((current + delta).clamp(0, last as isize) as usize);
    }

    /// Case-insensitive substring match over the title and subtitle. Selection
    /// returns to the top, because the row it pointed at may be gone.
    fn refilter(&mut self) {
        let needle = self.query.to_lowercase();
        self.shown = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| needle.is_empty() || row.filter.contains(&needle))
            .map(|(index, _)| index)
            .collect();
        let has_selection = self.style == PickerStyle::Choose && !self.shown.is_empty();
        self.selected = has_selection.then_some(0);
    }
}

/// What a key meant to the overlay that owns the picker.
pub(super) enum PickerKey {
    Consumed,
    Close,
    Accept,
    AcceptAll,
}

/// Apply an already-resolved picker action. While filtering, unmatched
/// printable keys extend the query, so command letters remain typeable.
pub(super) fn picker_key(picker: &mut Picker, action: Option<Action>, k: KeyEvent) -> PickerKey {
    if picker.typing {
        return match action {
            Some(Action::StopFilter) => {
                picker.typing = false;
                PickerKey::Consumed
            }
            Some(Action::FilterAcceptAll) => PickerKey::AcceptAll,
            Some(Action::FilterAccept) => PickerKey::Accept,
            Some(Action::FilterBackspace) => {
                picker.query.pop();
                picker.refilter();
                PickerKey::Consumed
            }
            Some(Action::FilterNext) => {
                picker.step(1);
                PickerKey::Consumed
            }
            Some(Action::FilterPrevious) => {
                picker.step(-1);
                PickerKey::Consumed
            }
            None if matches!(k.code, KeyCode::Char(_)) && typed(k.modifiers) => {
                let KeyCode::Char(c) = k.code else {
                    unreachable!()
                };
                picker.query.push(c);
                picker.refilter();
                PickerKey::Consumed
            }
            _ => PickerKey::Consumed,
        };
    }
    match action {
        Some(Action::ClosePicker) => PickerKey::Close,
        Some(Action::StartFilter) => {
            picker.typing = true;
            PickerKey::Consumed
        }
        Some(Action::PickerNext) => {
            picker.step(1);
            PickerKey::Consumed
        }
        Some(Action::PickerPrevious) => {
            picker.step(-1);
            PickerKey::Consumed
        }
        Some(Action::PickerAcceptAll) => PickerKey::AcceptAll,
        Some(Action::PickerAccept) => PickerKey::Accept,
        _ => PickerKey::Consumed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::test_support::*;

    #[test]
    fn slash_filters_a_picker_case_insensitively() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        app.interaction = Interaction::Move(Picker::new(vec![
            PickerRow::new("Inbox".into(), None, "l1".into()),
            PickerRow::new("Tinker Time".into(), None, "l2".into()),
            PickerRow::new("Writing".into(), None, "l3".into()),
        ]));

        // Without a query every row is offered.
        assert_eq!(app.interaction.picker().unwrap().rows().count(), 3);

        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert!(app.interaction.picker().unwrap().typing());
        typed_keys(&mut app, "TIN");

        // Substring, not fuzzy, and not anchored: "wriTINg" matches too. Case
        // is folded on both sides.
        let picker = app.interaction.picker().unwrap();
        assert_eq!(picker.query(), "TIN");
        let titles: Vec<&str> = picker.rows().map(|row| row.title.as_str()).collect();
        assert_eq!(titles, ["Tinker Time", "Writing"]);
        // Selection follows the filter rather than pointing at a hidden row.
        assert_eq!(picker.selected().map(|row| row.value.as_str()), Some("l2"));

        typed_keys(&mut app, "ker");
        let picker = app.interaction.picker().unwrap();
        let titles: Vec<&str> = picker.rows().map(|row| row.title.as_str()).collect();
        assert_eq!(titles, ["Tinker Time"]);

        // Backspace widens it again.
        for _ in 0..6 {
            app.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        }
        assert_eq!(app.interaction.picker().unwrap().rows().count(), 3);
    }

    #[test]
    fn a_filter_that_matches_nothing_has_no_selection_to_act_on() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        app.interaction = Interaction::Move(Picker::new(vec![PickerRow::new(
            "Inbox".into(),
            None,
            "l1".into(),
        )]));
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        typed_keys(&mut app, "zzz");
        let picker = app.interaction.picker().unwrap();
        assert!(picker.is_empty());
        assert!(picker.selected().is_none());
        // Enter cannot act on nothing, so the overlay simply stays put.
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Move(_)));
    }

    #[test]
    fn letters_type_into_the_query_instead_of_closing_the_picker() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        app.interaction = Interaction::Move(Picker::new(vec![PickerRow::new(
            "quilting".into(),
            None,
            "l1".into(),
        )]));
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        // `q` and `j` would close or move the list when not filtering.
        typed_keys(&mut app, "qj");
        assert!(matches!(app.interaction, Interaction::Move(_)));
        assert_eq!(app.interaction.picker().unwrap().query(), "qj");

        // Esc leaves the filter but keeps the overlay and the query.
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.interaction.picker().unwrap().typing());
        assert_eq!(app.interaction.picker().unwrap().query(), "qj");
        // A second Esc closes it.
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.interaction, Interaction::Idle));
    }

    #[test]
    fn the_keyboard_table_filters_but_never_selects() {
        let (mut app, _cmds) = app_with_lists(&["a"]);
        app.on_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(app.interaction.picker().unwrap().selected().is_none());
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        typed_keys(&mut app, "new subtask");
        let picker = app.interaction.picker().unwrap();
        assert_eq!(picker.rows().count(), 1);
        assert_eq!(picker.rows().next().unwrap().title, "shift+c");
        assert!(
            picker.selected().is_none(),
            "the reference card has no action"
        );
    }
}
