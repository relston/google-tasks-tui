# omarchy-tui

A fast Rust TUI (ratatui 0.29 + crossterm) that fully integrates with the
Omarchy theming system and live-reloads when the user switches themes.

## Layout
- `src/theme.rs` — finds the active Omarchy theme, parses `colors.toml`, maps it
  to semantic roles (`bg, fg, surface, accent, muted, sel_bg, sel_fg, red,
  green, yellow, blue`). Exposes `load()` and `fingerprint()`.
- `src/main.rs` — demo app: header w/ theme name, colored event list, status
  bar. Event loop polls input with a 500 ms timeout and calls `check_theme()`.

## Omarchy theming facts (verified Sept 2026)
- Active theme dir, current releases: `$XDG_STATE_HOME/omarchy/current`
  (default `~/.local/state/omarchy/current`). Legacy: `~/.config/omarchy/current`.
  Always check state dir first, fall back to config.
- Palette: `<current>/theme/colors.toml`, flat `key = "#rrggbb"`.
- Current keys are named: `mode`, `accent`, `selection`, `muted`,
  `background`, `dark_background`, `darker_background`, `lighter_background`,
  `foreground`, `dark_foreground`, `light_foreground`, `bright_foreground`,
  `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `bright_*`.
- Legacy keys: `accent, cursor, foreground, background, selection_foreground,
  selection_background, color0..color15`. Support both; named keys win.
- Theme switches fire hooks in `~/.config/omarchy/hooks/theme-set.d/*.sh`.
- Users can also add `~/.config/omarchy/themed/<file>.tpl` templates with
  `{{ accent }}`-style placeholders (plus `_strip`, `_rgb` modifiers) that
  Omarchy regenerates on every theme switch.

## Design rules
- Off-Omarchy (or unparseable file) → fall back to ANSI colors (`Color::Reset`
  etc.) so the app matches any terminal. Never crash on missing theme.
- Reload only when the fingerprint (symlink target + colors.toml mtime)
  changes; keep the idle loop cheap.
- Transparent mode paints `Color::Reset` backgrounds so terminal opacity
  shows through (`t` toggles).

## Commands
- `cargo run --release` / `cargo test`
- Test in `theme.rs` builds a fake state dir via `HOME` / `XDG_STATE_HOME`.

## Next steps
1. Add a `theme-set.d` hook script (installed by the app or a `--install-hook`
   flag) that signals running instances (e.g. SIGUSR1 or a unix socket) for
   instant reload; keep polling as fallback.
2. Honor `mode = "light"` / `light.mode` if any role needs light-mode tweaks.
3. Consider `toml` 1.x and ratatui latest if MSRV allows.
