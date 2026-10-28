//! The status bar: a pending confirmation, the latest error or notice, or
//! hints built from the active keymap.

use ratatui::{layout::Rect, style::Style, text::Line, Frame};

use crate::keys::Action;
use crate::theme::Theme;

use super::super::app::{App, Interaction, QuitState};
use super::super::editor::FieldId;
use super::super::picker::Picker;
use super::super::sync::SyncState;

impl App {
    fn hints(&self, commands: &[(Action, &str)]) -> String {
        let hints = commands
            .iter()
            .filter_map(|(action, description)| {
                let labels = self.keymap.labels_for(*action);
                (!labels.is_empty()).then(|| format!("{} {description}", labels.join("/")))
            })
            .collect::<Vec<_>>();
        if hints.is_empty() {
            " no commands bound".into()
        } else {
            format!(" {}", hints.join(" · "))
        }
    }

    pub(super) fn draw_status(&self, f: &mut Frame, status: Rect, t: &Theme) {
        let hints = if self.quit == QuitState::Waiting {
            let quit = self.keymap.labels_for(Action::Quit).join("/");
            format!(" waiting for pending saves… {quit} again to force quit")
        } else if self.interaction.picker().is_some_and(Picker::typing) {
            self.hints(&[
                (Action::FilterNext, "next"),
                (Action::FilterPrevious, "previous"),
                (Action::FilterAccept, "pick"),
                (Action::FilterAcceptAll, "all"),
                (Action::StopFilter, "stop filtering"),
            ])
        } else if matches!(self.interaction, Interaction::Browse(_)) {
            self.hints(&[
                (Action::PickerNext, "next"),
                (Action::PickerPrevious, "previous"),
                (Action::PickerAccept, "open"),
                (Action::PickerAcceptAll, "all"),
                (Action::StartFilter, "filter"),
                (Action::ClosePicker, "close"),
            ])
        } else if matches!(self.interaction, Interaction::Move(_)) {
            self.hints(&[
                (Action::PickerNext, "next"),
                (Action::PickerPrevious, "previous"),
                (Action::PickerAccept, "move"),
                (Action::StartFilter, "filter"),
                (Action::ClosePicker, "close"),
            ])
        } else if matches!(self.interaction, Interaction::Parent(_)) {
            self.hints(&[
                (Action::PickerNext, "next"),
                (Action::PickerPrevious, "previous"),
                (Action::PickerAccept, "choose parent"),
                (Action::StartFilter, "filter"),
                (Action::ClosePicker, "close"),
            ])
        } else if matches!(self.interaction, Interaction::Help(_)) {
            self.hints(&[
                (Action::StartFilter, "filter"),
                (Action::ClosePicker, "close"),
            ])
        } else if self.is_creating() {
            self.hints(&[(Action::SaveEdit, "create"), (Action::CancelEdit, "cancel")])
        } else if self.edit().is_some_and(|e| e.field() == FieldId::Notes) {
            self.hints(&[
                (Action::SaveEdit, "save"),
                (Action::InsertNewline, "new line"),
                (Action::CancelEdit, "cancel"),
                (Action::DeleteWord, "delete word"),
            ])
        } else if self.edit().is_some() {
            self.hints(&[
                (Action::SaveEdit, "save"),
                (Action::CancelEdit, "cancel"),
                (Action::DeleteWord, "delete word"),
                (Action::ClearToLineStart, "clear"),
            ])
        } else if self.sync != SyncState::Ready {
            " syncing with Google… board is read-only until it lands".into()
        } else {
            self.hints(&[(Action::Help, "keyboard shortcuts")])
        };
        let pending = match &self.interaction {
            Interaction::ConfirmDelete(pending) => Some(pending),
            _ => None,
        };
        let status_line = match (pending, &self.error, &self.notice) {
            (Some(p), _, _) => Line::styled(
                format!(" {}", p.prompt),
                Style::default().fg(t.yellow).bold(),
            ),
            (None, Some(e), _) => Line::styled(format!(" {e}"), Style::default().fg(t.red)),
            (None, None, Some(n)) => Line::styled(format!(" {n}"), Style::default().fg(t.green)),
            (None, None, None) => Line::styled(hints, Style::default().fg(t.subtle)),
        };
        f.render_widget(status_line, status);
    }
}
