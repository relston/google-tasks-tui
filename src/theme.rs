//! Omarchy theme loader. Reads the active theme's colors.toml and maps it to
//! semantic roles. Supports both the current layout
//! (~/.local/state/omarchy/current, named keys like `red`, `muted`) and the
//! older one (~/.config/omarchy/current, `color0..color15`). Off-Omarchy it
//! falls back to plain ANSI colors, so the terminal's own palette shows through.

use std::{collections::HashMap, env, fs, path::PathBuf, time::SystemTime};

use ratatui::style::Color;

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
    /// ANSI fallback: inherits whatever the terminal is themed with.
    fn default() -> Self {
        Self {
            name: "terminal (ANSI)".into(),
            bg: Color::Reset,
            fg: Color::Reset,
            surface: Color::Reset,
            accent: Color::Blue,
            muted: Color::DarkGray,
            sel_bg: Color::Blue,
            sel_fg: Color::Black,
            red: Color::Red,
            green: Color::Green,
            yellow: Color::Yellow,
            blue: Color::Blue,
        }
    }
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
    if s.len() < 6 {
        return None;
    }
    let n = u32::from_str_radix(&s[..6], 16).ok()?;
    Some(Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

pub fn load() -> Theme {
    Roots::from_env()
        .and_then(|roots| try_load(&roots))
        .unwrap_or_default()
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
        red: pick(&["red", "color1"], d.red),
        green: pick(&["green", "color2"], d.green),
        yellow: pick(&["yellow", "color3"], d.yellow),
        blue: pick(&["blue", "color4"], d.blue),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "accent = \"#81a1c1\"\nbackground = \"#2e3440\"\nforeground = \"#d8dee9\"\nred = \"#bf616a\"\ncolor2 = \"#a3be8c\"\n",
        )
        .unwrap();
        let t = try_load(&Roots {
            home: root.join("home"),
            state: root.join("state"),
        })
        .unwrap();
        assert_eq!(t.name, "nord");
        assert_eq!(t.accent, Color::Rgb(0x81, 0xa1, 0xc1));
        assert_eq!(t.red, Color::Rgb(0xbf, 0x61, 0x6a)); // named key
        assert_eq!(t.green, Color::Rgb(0xa3, 0xbe, 0x8c)); // legacy fallback
        assert_eq!(t.surface, Color::Rgb(0x2e, 0x34, 0x40)); // falls back to bg
        fs::remove_dir_all(root).ok();
    }
}
