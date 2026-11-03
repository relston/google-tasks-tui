//! Resolving keys to actions per context. User bindings replace an
//! action's defaults; conflicts and unsafe bindings become diagnostics.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use serde::{Deserialize, Serialize};

use super::actions::{Action, BindingContext};
use super::combo::{key_event_matches_combo, normalize_key_combo, parse_binding, ResolvedBinding};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Default,
    User,
}

#[derive(Clone, Debug)]
struct Entry {
    context: BindingContext,
    action: Action,
    source: Source,
    binding: ResolvedBinding,
}

#[derive(Clone, Debug)]
pub struct Keymap {
    entries: Vec<Entry>,
}

impl Keymap {
    /// The default bindings, with each override replacing its action's
    /// defaults in every context the action works in. User bindings are
    /// placed first, so they win over a conflicting default.
    pub fn new(overrides: &[(Action, BindingConfig)]) -> (Self, Vec<String>) {
        let user = |action: Action| {
            overrides
                .iter()
                .find(|(candidate, _)| *candidate == action)
                .map(|(_, config)| config)
        };
        let mut entries: Vec<Entry> = Vec::new();
        let mut diagnostics = Vec::new();

        for source in [Source::User, Source::Default] {
            for &action in Action::ALL {
                let values = match (source, user(action)) {
                    (Source::User, Some(config)) => config.values(),
                    (Source::Default, None) => action.defaults().to_vec(),
                    _ => continue,
                };
                for &context in action.contexts() {
                    for raw in &values {
                        let raw = raw.trim();
                        if raw.is_empty() {
                            continue;
                        }
                        let binding = match parse_binding(raw) {
                            Ok(binding) => binding,
                            Err(error) => {
                                diagnostics.push(format!(
                                    "invalid keybinding: {} = {raw:?}: {error}; disabling binding",
                                    action.name()
                                ));
                                continue;
                            }
                        };

                        if matches!(
                            context,
                            BindingContext::Editor
                                | BindingContext::Filter
                                | BindingContext::DateEdit
                                | BindingContext::ListEdit
                        ) && matches!(
                            binding.combo,
                            (KeyCode::Char(_), modifiers) if modifiers.is_empty()
                        ) {
                            diagnostics.push(format!(
                                "unsafe keybinding: {} = {raw:?} would intercept typing; disabling binding",
                                action.name()
                            ));
                            continue;
                        }

                        let conflict = entries.iter().find(|entry| {
                            entry.context == context
                                && normalize_key_combo(entry.binding.combo)
                                    == normalize_key_combo(binding.combo)
                        });
                        if let Some(conflict) = conflict {
                            if source == Source::Default && conflict.source == Source::User {
                                continue;
                            }
                            diagnostics.push(format!(
                                "{}: kept {}, disabled {}",
                                binding.label,
                                conflict.action.name(),
                                action.name()
                            ));
                            continue;
                        }

                        entries.push(Entry {
                            context,
                            action,
                            source,
                            binding,
                        });
                    }
                }
            }
        }

        diagnostics.dedup();
        (Self { entries }, diagnostics)
    }

    pub fn resolve(&self, context: BindingContext, key: &KeyEvent) -> Option<Action> {
        self.entries
            .iter()
            .find(|entry| {
                entry.context == context && key_event_matches_combo(key, entry.binding.combo)
            })
            .map(|entry| entry.action)
    }

    /// The active keys for an action, once each, for help and hints.
    pub fn labels_for(&self, action: Action) -> Vec<String> {
        let mut labels = Vec::new();
        for entry in self.entries.iter().filter(|entry| entry.action == action) {
            if !labels.contains(&entry.binding.label) {
                labels.push(entry.binding.label.clone());
            }
        }
        labels
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    #[derive(Deserialize)]
    struct BindingFixture {
        binding: BindingConfig,
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
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
    fn a_user_binding_displaces_a_default_only_in_its_own_context() {
        let (keymap, diagnostics) = Keymap::new(&[(Action::Refresh, BindingConfig::one("y"))]);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");

        assert_eq!(
            keymap.resolve(BindingContext::Board, &key('y')),
            Some(Action::Refresh)
        );
        assert!(keymap.labels_for(Action::CopyTask).is_empty());
        assert_eq!(
            keymap.resolve(BindingContext::Confirm, &key('y')),
            Some(Action::ConfirmDelete)
        );
    }

    #[test]
    fn conflicting_user_bindings_keep_the_first_and_report_the_second() {
        let (keymap, diagnostics) = Keymap::new(&[
            (Action::Refresh, BindingConfig::one("x")),
            (Action::Quit, BindingConfig::one("x")),
        ]);
        assert_eq!(diagnostics, ["x: kept quit, disabled refresh"]);
        assert_eq!(
            keymap.resolve(BindingContext::Board, &key('x')),
            Some(Action::Quit)
        );
    }

    #[test]
    fn empty_bindings_disable_an_action_without_diagnostics() {
        let (keymap, diagnostics) = Keymap::new(&[(Action::CreateTask, BindingConfig::empty())]);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert!(keymap.labels_for(Action::CreateTask).is_empty());
        assert_eq!(keymap.resolve(BindingContext::Board, &key('c')), None);
    }

    #[test]
    fn text_entry_contexts_reject_unmodified_printable_commands() {
        let (keymap, diagnostics) = Keymap::new(&[
            (Action::SaveEdit, BindingConfig::one("x")),
            (Action::StopFilter, BindingConfig::one("y")),
        ]);
        assert!(keymap.labels_for(Action::SaveEdit).is_empty());
        assert!(keymap.labels_for(Action::StopFilter).is_empty());
        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics
            .iter()
            .all(|message| message.contains("would intercept typing")));
    }
}
