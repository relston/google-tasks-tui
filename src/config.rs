//! User settings at `<config dir>/config.toml`, next to the token and the
//! board snapshot. The file is written with its defaults the first time the
//! app runs, so it documents itself and can be edited in place.

use std::{fs, io};

use serde::Deserialize;

use crate::{paths::Paths, storage};

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
#[serde(deny_unknown_fields)]
pub struct Config {
    pub wrap_titles: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self { wrap_titles: true }
    }
}

/// The saved settings, writing the default file first when there isn't one.
pub fn load() -> Config {
    load_with_paths(&Paths::from_env()).0
}

pub fn load_with_paths(paths: &Paths) -> (Config, Option<String>) {
    let path = paths.config();
    match fs::read_to_string(&path) {
        Ok(raw) => match parse(&raw) {
            Ok(config) => (config, None),
            Err(error) => (
                Config::default(),
                Some(format!("{}: {error}", path.display())),
            ),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let warning = storage::atomic_private_write(&path, TEMPLATE.as_bytes())
                .err()
                .map(|error| format!("writing {}: {error}", path.display()));
            (Config::default(), warning)
        }
        Err(error) => (
            Config::default(),
            Some(format!("reading {}: {error}", path.display())),
        ),
    }
}

fn parse(raw: &str) -> Result<Config, toml::de::Error> {
    toml::from_str(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn template_matches_the_defaults() {
        assert_eq!(parse(TEMPLATE).unwrap(), Config::default());
    }

    #[test]
    fn wrapping_is_on_unless_it_is_turned_off() {
        assert!(Config::default().wrap_titles);
        assert!(!parse("wrap_titles = false").unwrap().wrap_titles);
        assert!(parse("wrap_titles = true").unwrap().wrap_titles);
    }

    #[test]
    fn malformed_and_unknown_settings_are_rejected() {
        assert_eq!(parse("").unwrap(), Config::default());
        for raw in ["not toml at all", "wrap_titles = \"yes\"", "nonsense = 1"] {
            assert!(parse(raw).is_err(), "{raw:?}");
        }
    }

    #[test]
    fn filesystem_errors_and_parse_errors_are_visible() {
        let root = std::env::temp_dir().join(format!(
            "gtt-config-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = Paths {
            credentials: root.join("credentials.json"),
            config_dir: root.join("config"),
        };

        let (_, warning) = load_with_paths(&paths);
        assert!(warning.is_none());
        assert!(paths.config().exists());

        fs::write(paths.config(), "wrap_titles = \"maybe\"").unwrap();
        let (_, warning) = load_with_paths(&paths);
        assert!(warning.is_some_and(|warning| {
            warning.contains("config.toml") && warning.contains("invalid type")
        }));
        fs::remove_dir_all(root).ok();
    }
}
