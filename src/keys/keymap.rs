//! Resolving keys to actions per context, with user overrides and
//! conflict diagnostics.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use serde::{Deserialize, Serialize};

use super::actions::{default_binding_specs, Action, BindingContext};
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

    pub fn resolve(&self, context: BindingContext, key: &KeyEvent) -> Option<A> {
        self.matching(context, key)
            .map(|entry| entry.action.clone())
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

pub fn default_keymap() -> (Keymap<Action>, Vec<String>) {
    Keymap::build(&default_binding_specs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

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
            Some(TestAction::Help)
        );
        assert_eq!(
            keymap.resolve(BindingContext::Picker, &c),
            Some(TestAction::Pick)
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
            Some(TestAction::New)
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
