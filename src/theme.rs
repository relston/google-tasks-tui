//! Theme resolution in three layers: terminal ANSI defaults, then the active
//! Omarchy palette when `source = "auto"` finds one, then `[theme]` overrides
//! from `config.toml`. The result is finally fitted to what the terminal can
//! show: `NO_COLOR` drops color, and 24-bit values fall back to the 256- or
//! 16-color palette when truecolor isn't advertised.
//!
//! Red, green, yellow, and blue stay ANSI even on Omarchy: Omarchy themes the
//! terminal's palette with the same values, so indices look identical there
//! and keep working everywhere else. Only roles with no ANSI equivalent
//! (background, surface, accent, muted, selection) are read from Omarchy's
//! `colors.toml`, which exists in the current layout
//! (~/.local/state/omarchy/current, named keys like `accent`, `muted`) and the
//! older one (~/.config/omarchy/current, `color0..color15`).

use std::{collections::HashMap, env, ffi::OsStr, fmt, fs, path::PathBuf, time::SystemTime};

use ratatui::style::{Color, Style};
use serde::{de, Deserialize, Deserializer};

struct Roots {
    home: PathBuf,
    state: PathBuf,
}

impl Roots {
    fn from_env() -> Option<Self> {
        let home = PathBuf::from(env::var_os("HOME")?);
        let state = env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/state"));
        Some(Self { home, state })
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub name: String,
    pub bg: Color,
    pub fg: Color,
    pub surface: Color,
    pub accent: Color,
    pub muted: Color,
    pub sel_bg: Color,
    pub sel_fg: Color,
    pub red: Color,
    pub green: Color,
    pub yellow: Color,
    pub blue: Color,
}

impl Default for Theme {
    /// ANSI fallback: inherits whatever the terminal is themed with. It
    /// paints no background or text color of its own, and the selection and
    /// header badge use reverse video, so it reads correctly on light and
    /// dark terminals alike without detecting which one it is on.
    fn default() -> Self {
        Self {
            name: "terminal (ANSI)".into(),
            bg: Color::Reset,
            fg: Color::Reset,
            surface: Color::Reset,
            accent: Color::Blue,
            muted: Color::DarkGray,
            sel_bg: Color::Reset,
            sel_fg: Color::Reset,
            red: Color::Red,
            green: Color::Green,
            yellow: Color::Yellow,
            blue: Color::Blue,
        }
    }
}

impl Theme {
    /// The highlight for selected rows and active inputs. Without a
    /// selection background (`NO_COLOR`, or `selection = "reset"`) it
    /// reverses video so the selection stays visible.
    pub fn selection(&self) -> Style {
        let style = Style::default().fg(self.sel_fg).bg(self.sel_bg);
        if self.sel_bg == Color::Reset {
            style.reversed()
        } else {
            style
        }
    }

    /// The header badge: the background color on the accent. Without a
    /// painted background there is no known color to put on the accent, so
    /// it reverses the terminal's own colors instead.
    pub fn badge(&self) -> Style {
        if self.bg == Color::Reset {
            Style::default().reversed()
        } else {
            Style::default().fg(self.bg).bg(self.accent)
        }
    }

    fn roles_mut(&mut self) -> [&mut Color; 11] {
        [
            &mut self.bg,
            &mut self.fg,
            &mut self.surface,
            &mut self.accent,
            &mut self.muted,
            &mut self.sel_bg,
            &mut self.sel_fg,
            &mut self.red,
            &mut self.green,
            &mut self.yellow,
            &mut self.blue,
        ]
    }

    fn fit(mut self, support: ColorSupport) -> Self {
        for color in self.roles_mut() {
            *color = support.fit(*color);
        }
        if support == ColorSupport::NoColor {
            self.name = "none (NO_COLOR)".into();
        }
        self
    }
}

/// Where the base palette comes from before `[theme]` overrides apply.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The active Omarchy theme when one is installed, otherwise ANSI.
    #[default]
    Auto,
    /// Always the terminal's own ANSI palette.
    Ansi,
}

/// The `[theme]` table in `config.toml`. Unset roles keep the source's value.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub struct ThemeConfig {
    pub source: Source,
    pub background: Option<ColorSpec>,
    pub foreground: Option<ColorSpec>,
    pub surface: Option<ColorSpec>,
    pub accent: Option<ColorSpec>,
    pub muted: Option<ColorSpec>,
    pub selection: Option<ColorSpec>,
    pub selection_foreground: Option<ColorSpec>,
    pub red: Option<ColorSpec>,
    pub green: Option<ColorSpec>,
    pub yellow: Option<ColorSpec>,
    pub blue: Option<ColorSpec>,
}

impl ThemeConfig {
    /// Every role key, in the order the config template documents them.
    pub const ROLES: [&'static str; 11] = [
        "background",
        "foreground",
        "surface",
        "accent",
        "muted",
        "selection",
        "selection_foreground",
        "red",
        "green",
        "yellow",
        "blue",
    ];

    fn apply(&self, mut theme: Theme) -> Theme {
        let overrides = [
            self.background,
            self.foreground,
            self.surface,
            self.accent,
            self.muted,
            self.selection,
            self.selection_foreground,
            self.red,
            self.green,
            self.yellow,
            self.blue,
        ];
        for (color, spec) in theme.roles_mut().into_iter().zip(overrides) {
            if let Some(ColorSpec(value)) = spec {
                *color = value;
            }
        }
        theme
    }
}

/// A configured color: an ANSI name (`red`, `bright-black`), a palette index
/// (`0`–`255`), `"#rrggbb"`, or `"reset"` for the terminal default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorSpec(pub Color);

impl<'de> Deserialize<'de> for ColorSpec {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl de::Visitor<'_> for Visitor {
            type Value = ColorSpec;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an ANSI color name, a 0-255 palette index, \"#rrggbb\", or \"reset\"")
            }

            fn visit_i64<E: de::Error>(self, n: i64) -> Result<ColorSpec, E> {
                u8::try_from(n)
                    .map(|n| ColorSpec(Color::Indexed(n)))
                    .map_err(|_| E::custom(format!("palette index {n} is outside 0-255")))
            }

            fn visit_u64<E: de::Error>(self, n: u64) -> Result<ColorSpec, E> {
                self.visit_i64(i64::try_from(n).unwrap_or(i64::MAX))
            }

            fn visit_str<E: de::Error>(self, s: &str) -> Result<ColorSpec, E> {
                parse_color(s)
                    .map(ColorSpec)
                    .ok_or_else(|| E::custom(format!("unknown color {s:?}")))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(digits) = s.strip_prefix('#') {
        let valid = digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_hexdigit());
        return valid.then(|| hex(digits)).flatten();
    }
    Some(match s.to_ascii_lowercase().replace('_', "-").as_str() {
        "reset" | "default" => Color::Reset,
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        // ratatui names ANSI 7 `Gray` and ANSI 15 `White`.
        "white" => Color::Gray,
        "bright-black" | "gray" | "grey" => Color::DarkGray,
        "bright-red" => Color::LightRed,
        "bright-green" => Color::LightGreen,
        "bright-yellow" => Color::LightYellow,
        "bright-blue" => Color::LightBlue,
        "bright-magenta" => Color::LightMagenta,
        "bright-cyan" => Color::LightCyan,
        "bright-white" => Color::White,
        _ => return None,
    })
}

/// How many colors the terminal can show, from the usual environment
/// conventions: `NO_COLOR`, then `COLORTERM`, then `TERM`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSupport {
    NoColor,
    Ansi16,
    Ansi256,
    TrueColor,
}

impl ColorSupport {
    pub fn from_env() -> Self {
        Self::detect(
            env::var_os("NO_COLOR").as_deref(),
            env::var_os("COLORTERM").as_deref(),
            env::var_os("TERM").as_deref(),
        )
    }

    fn detect(no_color: Option<&OsStr>, colorterm: Option<&OsStr>, term: Option<&OsStr>) -> Self {
        // https://no-color.org: any non-empty value disables color.
        if no_color.is_some_and(|v| !v.is_empty()) {
            return Self::NoColor;
        }
        let colorterm = colorterm.and_then(OsStr::to_str).unwrap_or_default();
        if matches!(colorterm, "truecolor" | "24bit") {
            return Self::TrueColor;
        }
        // COLORTERM rarely survives ssh or sudo, but these terminfo names
        // only exist for truecolor terminals.
        let Some(term) = term.and_then(OsStr::to_str) else {
            return Self::Ansi256;
        };
        if term.ends_with("-direct")
            || matches!(
                term,
                "xterm-kitty" | "xterm-ghostty" | "alacritty" | "foot" | "wezterm" | "contour"
            )
        {
            Self::TrueColor
        } else if term.contains("256color") {
            Self::Ansi256
        } else {
            Self::Ansi16
        }
    }

    /// The closest color this terminal can show. ANSI names and `Reset`
    /// always pass through, since the terminal owns those.
    fn fit(self, color: Color) -> Color {
        match (self, color) {
            (Self::NoColor, _) => Color::Reset,
            (Self::TrueColor, c) => c,
            (Self::Ansi256, Color::Rgb(r, g, b)) => Color::Indexed(nearest_256((r, g, b))),
            (Self::Ansi16, Color::Rgb(r, g, b)) => nearest_16((r, g, b)),
            (Self::Ansi16, Color::Indexed(n)) if n >= 16 => nearest_16(indexed_rgb(n)),
            (Self::Ansi16, Color::Indexed(n)) => ANSI_16[usize::from(n)].0,
            (_, c) => c,
        }
    }
}

type Rgb = (u8, u8, u8);

/// The xterm 6×6×6 cube's channel levels (indices 16–231).
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// xterm's default values for the 16 base colors. Terminals retheme these,
/// so this is only a best guess for mapping a 24-bit color onto them.
const ANSI_16: [(Color, Rgb); 16] = [
    (Color::Black, (0, 0, 0)),
    (Color::Red, (205, 0, 0)),
    (Color::Green, (0, 205, 0)),
    (Color::Yellow, (205, 205, 0)),
    (Color::Blue, (0, 0, 238)),
    (Color::Magenta, (205, 0, 205)),
    (Color::Cyan, (0, 205, 205)),
    (Color::Gray, (229, 229, 229)),
    (Color::DarkGray, (127, 127, 127)),
    (Color::LightRed, (255, 0, 0)),
    (Color::LightGreen, (0, 255, 0)),
    (Color::LightYellow, (255, 255, 0)),
    (Color::LightBlue, (92, 92, 255)),
    (Color::LightMagenta, (255, 0, 255)),
    (Color::LightCyan, (0, 255, 255)),
    (Color::White, (255, 255, 255)),
];

fn distance(a: Rgb, b: Rgb) -> u32 {
    let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).unsigned_abs().pow(2);
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
}

fn indexed_rgb(n: u8) -> Rgb {
    match n {
        0..=15 => ANSI_16[usize::from(n)].1,
        16..=231 => {
            let i = n - 16;
            let level = |k: u8| CUBE[usize::from(k)];
            (level(i / 36), level(i / 6 % 6), level(i % 6))
        }
        _ => {
            let v = 8 + 10 * (n - 232);
            (v, v, v)
        }
    }
}

/// Nearest entry in the 256-color cube or gray ramp. The first 16 are
/// skipped because the terminal theme decides what they look like.
fn nearest_256(rgb: Rgb) -> u8 {
    (16..=255)
        .min_by_key(|&n| distance(rgb, indexed_rgb(n)))
        .unwrap_or(16)
}

fn nearest_16(rgb: Rgb) -> Color {
    ANSI_16
        .iter()
        .min_by_key(|(_, candidate)| distance(rgb, *candidate))
        .map_or(Color::Reset, |(color, _)| *color)
}

/// The directory Omarchy points at the active theme. Prefers the XDG state
/// location used by current releases, falling back to the legacy config path.
fn current_dir(roots: &Roots) -> Option<PathBuf> {
    [
        roots.state.join("omarchy/current"),
        roots.home.join(".config/omarchy/current"),
    ]
    .into_iter()
    .find(|p| p.exists())
}

fn colors_path(roots: &Roots) -> Option<PathBuf> {
    Some(current_dir(roots)?.join("theme/colors.toml"))
}

/// Cheap change fingerprint: the theme symlink's target plus the palette's
/// mtime. Covers both "symlink swapped" and "file regenerated in place".
pub type Fingerprint = Option<(Option<PathBuf>, Option<SystemTime>)>;

pub fn fingerprint() -> Fingerprint {
    fingerprint_with(&Roots::from_env()?)
}

fn fingerprint_with(roots: &Roots) -> Fingerprint {
    let dir = current_dir(roots)?;
    let link = fs::read_link(dir.join("theme"))
        .or_else(|_| fs::read_link(&dir))
        .ok();
    let mtime = colors_path(roots)
        .and_then(|p| fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());
    Some((link, mtime))
}

fn hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    if s.len() < 6 || !s.is_char_boundary(6) {
        return None;
    }
    let n = u32::from_str_radix(&s[..6], 16).ok()?;
    Some(Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

pub fn load(config: &ThemeConfig) -> Theme {
    let base = match config.source {
        Source::Auto => Roots::from_env().and_then(|roots| try_load(&roots)),
        Source::Ansi => None,
    };
    resolve(config, base, ColorSupport::from_env())
}

fn resolve(config: &ThemeConfig, base: Option<Theme>, support: ColorSupport) -> Theme {
    config.apply(base.unwrap_or_default()).fit(support)
}

fn try_load(roots: &Roots) -> Option<Theme> {
    let raw = fs::read_to_string(colors_path(roots)?).ok()?;
    let table: toml::Table = raw.parse().ok()?;
    let map: HashMap<&str, Color> = table
        .iter()
        .filter_map(|(k, v)| Some((k.as_str(), hex(v.as_str()?)?)))
        .collect();

    // First key present wins: canonical name, then legacy name, then default.
    let pick = |keys: &[&str], default: Color| {
        keys.iter()
            .find_map(|k| map.get(k).copied())
            .unwrap_or(default)
    };

    let d = Theme::default();
    let bg = pick(&["background", "color0"], d.bg);
    let fg = pick(&["foreground", "color7"], d.fg);
    let name = fingerprint_with(roots)
        .and_then(|(link, _)| link)
        .and_then(|p| {
            // `.../themes/<name>` or `.../themes/<name>/...`
            p.components()
                .skip_while(|c| c.as_os_str() != "themes")
                .nth(1)
                .or_else(|| p.components().next_back())
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "omarchy".into());

    Some(Theme {
        name,
        bg,
        fg,
        surface: pick(&["lighter_background", "color0"], bg),
        accent: pick(&["accent", "color4"], d.accent),
        muted: pick(&["muted", "dark_foreground", "color8"], d.muted),
        sel_bg: pick(&["selection", "selection_background"], d.sel_bg),
        sel_fg: pick(
            &["selection_foreground", "bright_foreground", "foreground"],
            fg,
        ),
        ..d
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> Result<ThemeConfig, toml::de::Error> {
        toml::from_str(raw)
    }

    #[test]
    fn loads_named_and_legacy_keys() {
        let root = env::temp_dir().join(format!(
            "google-tasks-tui-test-{}-{:?}",
            std::process::id(),
            SystemTime::now()
        ));
        let theme_dir = root.join("state/omarchy/themes/nord");
        fs::create_dir_all(&theme_dir).unwrap();
        fs::create_dir_all(root.join("state/omarchy/current")).unwrap();
        let _ = std::os::unix::fs::symlink(&theme_dir, root.join("state/omarchy/current/theme"));
        fs::write(
            theme_dir.join("colors.toml"),
            "accent = \"#81a1c1\"\nbackground = \"#2e3440\"\nforeground = \"#d8dee9\"\nred = \"#bf616a\"\ncolor8 = \"#4c566a\"\n",
        )
        .unwrap();
        let t = try_load(&Roots {
            home: root.join("home"),
            state: root.join("state"),
        })
        .unwrap();
        assert_eq!(t.name, "nord");
        assert_eq!(t.accent, Color::Rgb(0x81, 0xa1, 0xc1)); // named key
        assert_eq!(t.muted, Color::Rgb(0x4c, 0x56, 0x6a)); // legacy fallback
        assert_eq!(t.surface, Color::Rgb(0x2e, 0x34, 0x40)); // falls back to bg
        assert_eq!(t.red, Color::Red); // the terminal already has Omarchy's red
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn colors_accept_names_indices_hex_and_reset() {
        let config = parse(
            r##"
source = "ansi"
accent = "magenta"
muted = "bright-black"
selection = 61
selection_foreground = "#1E1E2e"
background = "reset"
blue = "Bright_Blue"
"##,
        )
        .unwrap();
        assert_eq!(config.source, Source::Ansi);
        assert_eq!(config.accent, Some(ColorSpec(Color::Magenta)));
        assert_eq!(config.muted, Some(ColorSpec(Color::DarkGray)));
        assert_eq!(config.selection, Some(ColorSpec(Color::Indexed(61))));
        assert_eq!(
            config.selection_foreground,
            Some(ColorSpec(Color::Rgb(0x1e, 0x1e, 0x2e)))
        );
        assert_eq!(config.background, Some(ColorSpec(Color::Reset)));
        assert_eq!(config.blue, Some(ColorSpec(Color::LightBlue)));
    }

    #[test]
    fn bad_colors_and_unknown_roles_are_rejected() {
        for raw in [
            "accent = \"chartreuse\"",
            "accent = \"#12345\"",
            "accent = \"#1234567\"",
            "accent = 256",
            "accent = -1",
            "accent = true",
            "source = \"omarchy\"",
            "orange = \"red\"",
        ] {
            assert!(parse(raw).is_err(), "{raw:?}");
        }
    }

    #[test]
    fn overrides_win_over_the_source_palette() {
        let omarchy = Theme {
            name: "nord".into(),
            accent: Color::Rgb(0x81, 0xa1, 0xc1),
            muted: Color::Rgb(0x4c, 0x56, 0x6a),
            ..Theme::default()
        };
        let config = parse("accent = \"yellow\"").unwrap();
        let t = resolve(&config, Some(omarchy), ColorSupport::TrueColor);
        assert_eq!(t.name, "nord");
        assert_eq!(t.accent, Color::Yellow);
        assert_eq!(t.muted, Color::Rgb(0x4c, 0x56, 0x6a));

        let t = resolve(&config, None, ColorSupport::TrueColor);
        assert_eq!(t.name, "terminal (ANSI)");
        assert_eq!(t.accent, Color::Yellow);
        assert_eq!(t.muted, Color::DarkGray);
    }

    #[test]
    fn support_follows_no_color_colorterm_and_term() {
        let detect = |no_color: Option<&str>, colorterm: Option<&str>, term: Option<&str>| {
            ColorSupport::detect(
                no_color.map(OsStr::new),
                colorterm.map(OsStr::new),
                term.map(OsStr::new),
            )
        };
        use ColorSupport::*;
        assert_eq!(detect(Some("1"), Some("truecolor"), None), NoColor);
        // An empty NO_COLOR does not count.
        assert_eq!(detect(Some(""), Some("truecolor"), None), TrueColor);
        assert_eq!(detect(None, Some("24bit"), Some("xterm")), TrueColor);
        assert_eq!(detect(None, None, Some("xterm-kitty")), TrueColor);
        assert_eq!(detect(None, None, Some("xterm-direct")), TrueColor);
        assert_eq!(detect(None, None, Some("tmux-256color")), Ansi256);
        assert_eq!(detect(None, None, Some("linux")), Ansi16);
        assert_eq!(detect(None, None, None), Ansi256);
    }

    #[test]
    fn colors_fit_what_the_terminal_supports() {
        let pink = Color::Rgb(0xb8, 0x55, 0x7d);
        assert_eq!(ColorSupport::TrueColor.fit(pink), pink);
        assert_eq!(ColorSupport::Ansi256.fit(pink), Color::Indexed(132));
        assert_eq!(
            ColorSupport::Ansi256.fit(Color::Rgb(0x13, 0x13, 0x13)),
            Color::Indexed(233)
        );
        assert_eq!(
            ColorSupport::Ansi16.fit(Color::Rgb(250, 10, 10)),
            Color::LightRed
        );
        assert_eq!(
            ColorSupport::Ansi16.fit(Color::Indexed(196)),
            Color::LightRed
        );
        assert_eq!(ColorSupport::Ansi16.fit(Color::Indexed(1)), Color::Red);
        // Names and the terminal default always pass through.
        assert_eq!(ColorSupport::Ansi16.fit(Color::Magenta), Color::Magenta);
        assert_eq!(ColorSupport::Ansi256.fit(Color::Reset), Color::Reset);
        assert_eq!(ColorSupport::NoColor.fit(pink), Color::Reset);
        assert_eq!(ColorSupport::NoColor.fit(Color::Red), Color::Reset);
    }

    #[test]
    fn no_color_keeps_the_selection_visible() {
        let t = resolve(&ThemeConfig::default(), None, ColorSupport::NoColor);
        assert_eq!(t.name, "none (NO_COLOR)");
        assert_eq!(t.accent, Color::Reset);
        assert_eq!(
            t.selection(),
            Style::default()
                .fg(Color::Reset)
                .bg(Color::Reset)
                .reversed()
        );

        assert_eq!(t.badge(), Style::default().reversed());
    }

    #[test]
    fn ansi_fallback_selection_and_badge_follow_the_terminal() {
        // Reverse video swaps the terminal's own foreground and background,
        // so it stays readable whether the terminal is light or dark.
        let t = Theme::default();
        assert_eq!(
            (t.bg, t.fg, t.surface),
            (Color::Reset, Color::Reset, Color::Reset)
        );
        assert_eq!(
            t.selection(),
            Style::default()
                .fg(Color::Reset)
                .bg(Color::Reset)
                .reversed()
        );
        assert_eq!(t.badge(), Style::default().reversed());

        let painted = Theme {
            bg: Color::Rgb(0xff, 0xff, 0xff),
            accent: Color::Rgb(0x1e, 0x66, 0xf5),
            sel_bg: Color::Rgb(0xcc, 0xd0, 0xda),
            sel_fg: Color::Rgb(0x4c, 0x4f, 0x69),
            ..Theme::default()
        };
        assert_eq!(
            painted.selection(),
            Style::default()
                .fg(Color::Rgb(0x4c, 0x4f, 0x69))
                .bg(Color::Rgb(0xcc, 0xd0, 0xda))
        );
        assert_eq!(
            painted.badge(),
            Style::default()
                .fg(Color::Rgb(0xff, 0xff, 0xff))
                .bg(Color::Rgb(0x1e, 0x66, 0xf5))
        );
    }
}
