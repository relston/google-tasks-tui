//! The action catalog, default bindings, key parsing, and per-context key
//! resolution. What each action does lives in the TUI's input handling.

mod actions;
mod combo;
mod keymap;

pub use actions::{default_binding_specs, Action, BindingContext};
pub use combo::{
    format_key_combo, key_event_matches_combo, normalize_key_combo, parse_binding, parse_key_combo,
    KeyCombo, ParseKeyError, ResolvedBinding,
};
pub use keymap::{default_keymap, BindingConfig, BindingSource, BindingSpec, Keymap};
