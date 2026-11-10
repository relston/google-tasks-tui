//! User settings at `<config dir>/config.toml`, next to the token and the
//! board snapshot. The file is written with its defaults the first time the
//! app runs, so it documents itself and can be edited in place.

use std::{collections::BTreeMap, fs, io};

use serde::Deserialize;

use crate::{
    atomic_file,
    keys::{Action, BindingConfig, Keymap},
    paths::Paths,
    theme::ThemeConfig,
};

/// The top of a fresh `config.toml`, through the `[keys]` header.
const HEADER: &str = "\
# google-tasks-tui settings. Delete a line to go back to its default.

# Wrap long task titles onto as many lines as they need, in the board columns
# and in the Browse Link overlay. `false` keeps every task on one line and
# clips what does not fit.
wrap_titles = true

# Keybindings accept one string or an array. An empty string or array disables
# an action. Supported modifiers include ctrl, shift, alt, and cmd/super;
# special keys include enter, esc, tab, arrows, home/end, and f1..f24.
[keys]
";

/// The bottom of a fresh `config.toml`: the theme roles.
const THEME: &str = "\
# Colors. `source = \"auto\"` follows the active Omarchy theme when one is
# installed and otherwise uses your terminal's own palette; \"ansi\" always
# uses the terminal palette. Any role below overrides the source. A color is
# an ANSI name (black, red, green, yellow, blue, magenta, cyan, white, or
# bright-<name>), a 0-255 palette index, \"#rrggbb\", or \"reset\" for the
# terminal default. Setting NO_COLOR turns color off entirely.
[theme]
# source = \"auto\"
# background = \"reset\"
# foreground = \"reset\"
# surface = \"reset\"
# accent = \"blue\"
# muted = \"bright-black\"
# subtle = \"bright-black\"
# selection = \"reset\"
# selection_foreground = \"reset\"
# red = \"red\"
# green = \"green\"
# yellow = \"yellow\"
# blue = \"blue\"
";

/// What ships in a fresh `config.toml`: every setting commented out at its
/// default. The `[keys]` section comes from the action table, so it cannot
/// drift from the real defaults.
pub fn default_toml() -> String {
    let mut out = String::from(HEADER);
    let mut section = None;
    for action in Action::ALL {
        let context = action.contexts()[0];
        if section != Some(context) {
            out.push_str(&format!("\n# {}\n", context.heading()));
            section = Some(context);
        }
        out.push_str(&format!(
            "# {} = {}\n",
            action.name(),
            toml_binding(action.defaults())
        ));
    }
    out.push('\n');
    out.push_str(THEME);
    out
}

fn toml_binding(keys: &[&str]) -> String {
    match keys {
        [one] => format!("{one:?}"),
        many => {
            let quoted: Vec<String> = many.iter().map(|key| format!("{key:?}")).collect();
            format!("[{}]", quoted.join(", "))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub wrap_titles: bool,
    /// Action name to binding. Names are checked when the keymap is built,
    /// so a typo is a warning rather than a reason to reject the file.
    pub keys: BTreeMap<String, BindingConfig>,
    pub theme: ThemeConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            wrap_titles: true,
            keys: BTreeMap::new(),
            theme: ThemeConfig::default(),
        }
    }
}

impl Config {
    pub fn keymap(&self) -> (Keymap, Vec<String>) {
        let mut diagnostics = Vec::new();
        let mut overrides = Vec::new();
        for (name, binding) in &self.keys {
            match Action::from_name(name) {
                Some(action) => overrides.push((action, binding.clone())),
                None => {
                    diagnostics.push(format!("unknown keybinding action {name:?}; ignoring it"))
                }
            }
        }
        let (keymap, binding_diagnostics) = Keymap::new(&overrides);
        diagnostics.extend(binding_diagnostics);
        (keymap, diagnostics)
    }
}

/// The saved settings, writing the default file first when there isn't one.
pub fn load(paths: &Paths) -> (Config, Option<String>) {
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
            let warning = atomic_file::write_private(&path, default_toml().as_bytes())
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
        assert_eq!(parse(&default_toml()).unwrap(), Config::default());
    }

    #[test]
    fn the_generated_keys_template_round_trips_to_the_default_keymap() {
        let template = default_toml();
        let keys_start = template.find("[keys]").unwrap();
        let keys_end = template.find("[theme]").unwrap();
        let uncommented: String = template[keys_start..keys_end]
            .lines()
            .map(|line| match line.strip_prefix("# ") {
                Some(rest)
                    if rest
                        .split_once(" = ")
                        .is_some_and(|(name, _)| Action::from_name(name).is_some()) =>
                {
                    rest
                }
                _ => line,
            })
            .map(|line| format!("{line}\n"))
            .collect();

        let config = parse(&uncommented).unwrap();
        assert_eq!(config.keys.len(), Action::ALL.len());
        let (keymap, diagnostics) = config.keymap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let (defaults, _) = Keymap::new(&[]);
        for action in Action::ALL {
            assert_eq!(
                keymap.labels_for(*action),
                defaults.labels_for(*action),
                "{}",
                action.name()
            );
        }
    }

    #[test]
    fn unknown_key_names_warn_while_other_settings_still_load() {
        let config = parse(
            r#"
wrap_titles = false

[keys]
nonsense = "x"
quit = "ctrl+q"

[theme]
accent = "magenta"
"#,
        )
        .unwrap();

        assert!(!config.wrap_titles);
        assert_eq!(
            config.theme.accent,
            Some(crate::theme::ColorSpec(ratatui::style::Color::Magenta))
        );
        let (keymap, diagnostics) = config.keymap();
        assert_eq!(keymap.labels_for(Action::Quit), ["ctrl+q"]);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert!(diagnostics[0].contains("nonsense"), "{diagnostics:?}");
    }

    #[test]
    fn wrapping_is_on_unless_it_is_turned_off() {
        assert!(Config::default().wrap_titles);
        assert!(!parse("wrap_titles = false").unwrap().wrap_titles);
        assert!(parse("wrap_titles = true").unwrap().wrap_titles);
    }

    #[test]
    fn malformed_and_unknown_top_level_settings_are_rejected() {
        assert_eq!(parse("").unwrap(), Config::default());
        for raw in [
            "not toml at all",
            "wrap_titles = \"yes\"",
            "nonsense = 1",
            "[keys]\nquit = 1",
            "[theme]\naccent = \"chartreuse\"",
        ] {
            assert!(parse(raw).is_err(), "{raw:?}");
        }
    }

    #[test]
    fn keys_accept_single_multiple_disabled_and_modified_bindings() {
        let config = parse(
            r#"
[keys]
create_task = ["n", "ctrl+b"]
delete_task = ""
"#,
        )
        .unwrap();

        assert_eq!(config.keys["create_task"].values(), ["n", "ctrl+b"]);
        assert!(config.keys["delete_task"].values().is_empty());
    }

    #[test]
    fn keymap_uses_overrides_disables_defaults_and_reports_bad_bindings() {
        let config = parse(
            r#"
[keys]
create_task = "n"
delete_task = ""
help = "not-a-key"
"#,
        )
        .unwrap();
        let (keymap, diagnostics) = config.keymap();

        assert_eq!(keymap.labels_for(crate::keys::Action::CreateTask), ["n"]);
        assert!(keymap
            .labels_for(crate::keys::Action::DeleteTask)
            .is_empty());
        assert!(keymap.labels_for(crate::keys::Action::Help).is_empty());
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics
            .iter()
            .any(|message| { message.contains("help") && message.contains("invalid keybinding") }));
    }

    #[test]
    fn configured_modified_chord_resolves_directly() {
        let config = parse(
            r#"
[keys]
create_task = "ctrl+b"
"#,
        )
        .unwrap();
        let (keymap, diagnostics) = config.keymap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(
            keymap.labels_for(crate::keys::Action::CreateTask),
            ["ctrl+b"]
        );
    }

    #[test]
    fn prefix_mode_is_not_an_action() {
        let (_, diagnostics) = parse("[keys]\nprefix = \"ctrl+b\"").unwrap().keymap();
        assert!(diagnostics.iter().any(|message| message.contains("prefix")));
    }

    #[test]
    fn reorder_keys_are_named_reorder_up_and_down() {
        let config = parse("[keys]\nreorder_up = \"ctrl+k\"\nreorder_down = \"ctrl+j\"").unwrap();
        let (keymap, diagnostics) = config.keymap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(keymap.labels_for(Action::ReorderUp), ["ctrl+k"]);
        assert_eq!(keymap.labels_for(Action::ReorderDown), ["ctrl+j"]);
        let (_, diagnostics) =
            parse("[keys]\nmove_task_up = \"ctrl+k\"\nmove_task_down = \"ctrl+j\"")
                .unwrap()
                .keymap();
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    }

    #[test]
    fn default_template_documents_every_configurable_action() {
        let template = default_toml();
        assert!(template.contains("[keys]"));
        for action in Action::ALL {
            assert!(
                template.contains(&format!("# {} =", action.name())),
                "missing {}",
                action.name()
            );
        }
    }

    #[test]
    fn default_template_documents_every_theme_role() {
        let template = default_toml();
        let theme = &template[template.find("[theme]").unwrap()..];
        assert!(theme.contains("source ="));
        for role in ThemeConfig::ROLES {
            assert!(theme.contains(&format!("# {role} =")), "missing {role}");
        }
    }

    #[test]
    fn theme_overrides_parse_alongside_keys() {
        let config = parse("[keys]\nquit = \"x\"\n[theme]\naccent = \"magenta\"").unwrap();
        assert_eq!(
            config.theme.accent,
            Some(crate::theme::ColorSpec(ratatui::style::Color::Magenta))
        );
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
            require_credentials_file: false,
            config_dir: root.join("config"),
        };

        let (_, warning) = load(&paths);
        assert!(warning.is_none());
        assert!(paths.config().exists());

        fs::write(paths.config(), "wrap_titles = \"maybe\"").unwrap();
        let (_, warning) = load(&paths);
        assert!(warning.is_some_and(|warning| {
            warning.contains("config.toml") && warning.contains("invalid type")
        }));
        fs::remove_dir_all(root).ok();
    }
}
