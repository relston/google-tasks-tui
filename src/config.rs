//! User settings at `<config dir>/config.toml`, next to the token and the
//! board snapshot. The file is written with its defaults the first time the
//! app runs, so it documents itself and can be edited in place.

use std::{fs, path::PathBuf};

use serde::Deserialize;

/// What ships in a fresh `config.toml`. Keep it in step with `Config`:
/// `template_matches_the_defaults` fails if it drifts.
const TEMPLATE: &str = "\
# google-tasks-tui settings. Delete a line to go back to its default.

# Wrap long task titles onto as many lines as they need, in the board columns
# and in the Browse Link overlay. `false` keeps every task on one line and
# clips what does not fit.
wrap_titles = true
";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Config {
    pub wrap_titles: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self { wrap_titles: true }
    }
}

fn path() -> PathBuf {
    crate::auth::config_dir().join("config.toml")
}

/// The saved settings, writing the default file first when there isn't one.
/// A file we can't read or parse falls back to the defaults and is left
/// alone, so a typo never costs anyone their settings.
pub fn load() -> Config {
    let path = path();
    match fs::read_to_string(&path) {
        Ok(raw) => parse(&raw),
        Err(_) => {
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::write(&path, TEMPLATE);
            Config::default()
        }
    }
}

fn parse(raw: &str) -> Config {
    toml::from_str(raw).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_matches_the_defaults() {
        assert_eq!(parse(TEMPLATE), Config::default());
    }

    #[test]
    fn wrapping_is_on_unless_it_is_turned_off() {
        assert!(Config::default().wrap_titles);
        assert!(!parse("wrap_titles = false").wrap_titles);
        assert!(parse("wrap_titles = true").wrap_titles);
    }

    #[test]
    fn a_broken_file_falls_back_to_the_defaults() {
        for raw in ["", "not toml at all", "wrap_titles = \"yes\"", "nonsense = 1"] {
            assert_eq!(parse(raw), Config::default(), "{raw:?}");
        }
    }
}
