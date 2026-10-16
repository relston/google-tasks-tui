//! User settings at `<config dir>/config.toml`, next to the token and the
//! board snapshot. The file is written with its defaults the first time the
//! app runs, so it documents itself and can be edited in place.

use std::{fs, io};

use serde::Deserialize;

use crate::{
    keybindings::{default_binding_specs, Action, BindingConfig, BindingSource, Keymap},
    paths::Paths,
    storage,
    theme::ThemeConfig,
};

/// What ships in a fresh `config.toml`. Keep it in step with `Config`:
/// `template_matches_the_defaults` fails if it drifts.
const TEMPLATE: &str = "\
# google-tasks-tui settings. Delete a line to go back to its default.

# Wrap long task titles onto as many lines as they need, in the board columns
# and in the Browse Link overlay. `false` keeps every task on one line and
# clips what does not fit.
wrap_titles = true

# Keybindings accept one string or an array. An empty string or array disables
# an action. Supported modifiers include ctrl, shift, alt, and cmd/super;
# special keys include enter, esc, tab, arrows, home/end, and f1..f24.
[keys]

# Board
# quit = \"q\"
# close_shelf = \"esc\"
# previous_list = [\"h\", \"left\"]
# next_list = [\"l\", \"right\"]
# previous_task = [\"k\", \"up\"]
# next_task = [\"j\", \"down\"]
# first_task = [\"g\", \"home\"]
# last_task = [\"shift+g\", \"end\"]
# toggle_details = \"enter\"
# next_field = \"tab\"
# previous_field = \"shift+tab\"
# edit = \"i\"
# toggle_done = \"space\"
# toggle_completed = \"v\"
# delete_task = \"d\"
# create_task = \"c\"
# create_subtask = \"shift+c\"
# refresh = \"r\"
# toggle_transparent = \"t\"
# copy_task = \"y\"
# browse_links = \"b\"
# move_task = \"m\"
# unparent_task = \"shift+comma\"
# choose_parent = \"shift+period\"
# help = \"shift+slash\"

# Picker overlays
# close_picker = [\"esc\", \"q\"]
# start_filter = \"slash\"
# picker_next = [\"j\", \"down\"]
# picker_previous = [\"k\", \"up\"]
# picker_accept = \"enter\"
# picker_accept_all = \"shift+enter\"

# Picker filter input
# stop_filter = \"esc\"
# filter_next = \"down\"
# filter_previous = \"up\"
# filter_accept = \"enter\"
# filter_accept_all = \"shift+enter\"
# filter_backspace = \"backspace\"

# Task editor
# cancel_edit = \"esc\"
# save_edit = \"enter\"
# insert_newline = [\"shift+enter\", \"alt+enter\", \"ctrl+j\"]
# delete_word = \"ctrl+w\"
# clear_to_line_start = \"ctrl+u\"
# cursor_line_start = [\"ctrl+a\", \"home\"]
# cursor_line_end = [\"ctrl+e\", \"end\"]
# cursor_left = \"left\"
# cursor_right = \"right\"
# cursor_up = \"up\"
# cursor_down = \"down\"
# editor_backspace = \"backspace\"
# editor_delete = \"delete\"

# Confirmation
# confirm_delete = \"y\"

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
# selection = \"reset\"
# selection_foreground = \"reset\"
# red = \"red\"
# green = \"green\"
# yellow = \"yellow\"
# blue = \"blue\"
";

pub fn default_toml() -> &'static str {
    TEMPLATE
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub wrap_titles: bool,
    pub keys: KeysConfig,
    pub theme: ThemeConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            wrap_titles: true,
            keys: KeysConfig::default(),
            theme: ThemeConfig::default(),
        }
    }
}

macro_rules! key_fields {
    ($($field:ident => $action:ident),+ $(,)?) => {
        #[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
        #[serde(default)]
        #[serde(deny_unknown_fields)]
        pub struct KeysConfig {
            $(pub $field: Option<BindingConfig>,)+
        }

        impl KeysConfig {
            fn binding(&self, action: Action) -> Option<&BindingConfig> {
                match action {
                    $(Action::$action => self.$field.as_ref(),)+
                }
            }
        }
    };
}

key_fields! {
    quit => Quit,
    close_shelf => CloseShelf,
    previous_list => PreviousList,
    next_list => NextList,
    previous_task => PreviousTask,
    next_task => NextTask,
    first_task => FirstTask,
    last_task => LastTask,
    toggle_details => ToggleDetails,
    next_field => NextField,
    previous_field => PreviousField,
    edit => Edit,
    toggle_done => ToggleDone,
    toggle_completed => ToggleCompleted,
    delete_task => DeleteTask,
    create_task => CreateTask,
    create_subtask => CreateSubtask,
    refresh => Refresh,
    toggle_transparent => ToggleTransparent,
    copy_task => CopyTask,
    browse_links => BrowseLinks,
    move_task => MoveTask,
    unparent_task => UnparentTask,
    choose_parent => ChooseParent,
    help => Help,
    close_picker => ClosePicker,
    start_filter => StartFilter,
    picker_next => PickerNext,
    picker_previous => PickerPrevious,
    picker_accept => PickerAccept,
    picker_accept_all => PickerAcceptAll,
    stop_filter => StopFilter,
    filter_next => FilterNext,
    filter_previous => FilterPrevious,
    filter_accept => FilterAccept,
    filter_accept_all => FilterAcceptAll,
    filter_backspace => FilterBackspace,
    cancel_edit => CancelEdit,
    save_edit => SaveEdit,
    insert_newline => InsertNewline,
    delete_word => DeleteWord,
    clear_to_line_start => ClearToLineStart,
    cursor_line_start => CursorLineStart,
    cursor_line_end => CursorLineEnd,
    cursor_left => CursorLeft,
    cursor_right => CursorRight,
    cursor_up => CursorUp,
    cursor_down => CursorDown,
    editor_backspace => EditorBackspace,
    editor_delete => EditorDelete,
    confirm_delete => ConfirmDelete,
}

impl Config {
    pub fn keymap(&self) -> (Keymap<Action>, Vec<String>) {
        let specs = default_binding_specs()
            .into_iter()
            .map(|mut spec| {
                if let Some(binding) = self.keys.binding(spec.action) {
                    spec.config = binding.clone();
                    spec.source = BindingSource::User;
                }
                spec
            })
            .collect::<Vec<_>>();
        Keymap::build(&specs)
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
        for raw in [
            "not toml at all",
            "wrap_titles = \"yes\"",
            "nonsense = 1",
            "[keys]\nnonsense = \"x\"",
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

        assert_eq!(
            config.keys.create_task.as_ref().unwrap().values(),
            ["n", "ctrl+b"]
        );
        assert!(config
            .keys
            .delete_task
            .as_ref()
            .unwrap()
            .values()
            .is_empty());
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

        assert_eq!(
            keymap.labels_for(crate::keybindings::Action::CreateTask),
            ["n"]
        );
        assert!(keymap
            .labels_for(crate::keybindings::Action::DeleteTask)
            .is_empty());
        assert!(keymap
            .labels_for(crate::keybindings::Action::Help)
            .is_empty());
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
            keymap.labels_for(crate::keybindings::Action::CreateTask),
            ["ctrl+b"]
        );
    }

    #[test]
    fn prefix_configuration_is_rejected() {
        assert!(parse("[keys]\nprefix = \"ctrl+b\"").is_err());
    }

    #[test]
    fn default_template_documents_every_configurable_action() {
        assert!(TEMPLATE.contains("[keys]"));
        for action in Action::ALL {
            assert!(
                TEMPLATE.contains(&format!("{} =", action.name())),
                "missing {}",
                action.name()
            );
        }
    }

    #[test]
    fn default_template_documents_every_theme_role() {
        let theme = &TEMPLATE[TEMPLATE.find("[theme]").unwrap()..];
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
