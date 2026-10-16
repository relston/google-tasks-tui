//! Key combinations: parsing config strings, matching terminal events,
//! and formatting labels.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fmt;

pub type KeyCombo = (KeyCode, KeyModifiers);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedBinding {
    pub combo: KeyCombo,
    pub label: String,
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

    fn event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
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
}
