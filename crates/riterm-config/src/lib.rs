//! Configuration for riterm, with iTerm2 compatibility importers.

pub mod colors;
pub mod iterm;
pub mod presets;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub use colors::Rgb;
pub use iterm::{ItermColors, ThemeMode, ThemeVariant, import_iterm2_prefs, parse_itermcolors};
pub use presets::{BUILTIN_PRESETS, BuiltinPreset};

/// Cursor shape selected by a profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorShapeSetting {
    #[default]
    Block,
    Beam,
    Underline,
    Hollow,
}

/// iTerm2's default ANSI palette (dark mode).
pub const ITERM2_DEFAULT_PALETTE: [&str; 16] = [
    "#14191e", "#b43c2a", "#00c200", "#c7c400", "#2744c7", "#c040be", "#00c5c7", "#c7c7c7",
    "#686868", "#dd7975", "#58e790", "#ece100", "#a7abf2", "#e17ee1", "#60fdff", "#ffffff",
];

/// Backwards-compatible alias for the default palette.
pub const DEFAULT_PALETTE: [&str; 16] = ITERM2_DEFAULT_PALETTE;

/// Action taken when an output trigger matches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerAction {
    /// Post a desktop notification.
    #[default]
    Notify,
    /// Mark the tab as needing attention (the same dot as BEL).
    Bell,
}

/// A regular expression matched against each line of terminal output.
///
/// Triggers are global in this release. `text` overrides the notification body;
/// when absent the matched line is used.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Trigger {
    pub regex: String,
    pub action: TriggerAction,
    pub text: Option<String>,
    pub enabled: bool,
}

impl Default for Trigger {
    fn default() -> Self {
        Self {
            regex: String::new(),
            action: TriggerAction::Notify,
            text: None,
            enabled: true,
        }
    }
}

/// Settings that apply to the application as a whole.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// Shell to run. Empty or absent uses the user's login shell.
    pub shell: Option<String>,
    /// Extra arguments for the shell.
    pub shell_args: Vec<String>,
    /// Startup directory. `~` is expanded.
    pub working_directory: Option<String>,
    /// Scrollback lines kept per session. `0` keeps unlimited history.
    pub scrollback: usize,
    pub default_profile: String,
    /// Height of the tab bar in logical pixels.
    pub tab_bar_height: f32,
    /// Padding inside each terminal pane in logical pixels.
    pub padding: f32,
    /// Copy the selection to the clipboard when the mouse is released.
    pub copy_on_select: bool,
    /// Paste the clipboard on middle click.
    pub middle_click_paste: bool,
    /// Master switch for desktop notifications.
    pub notifications: bool,
    /// Notify when a command finishes while the window is unfocused.
    pub notify_on_command_finish: bool,
    /// Notify when the terminal bell rings while the window is unfocused.
    pub notify_on_bell: bool,
    /// Minimum command runtime for finish notifications, in seconds.
    /// Failures notify regardless of duration.
    pub notification_min_seconds: f32,
    /// Built-in iTerm2 preset name or path to an `.itermcolors` file.
    pub theme: Option<String>,
    /// Which appearance variant of the theme to use.
    pub theme_variant: ThemeMode,
}

impl Default for General {
    fn default() -> Self {
        Self {
            shell: None,
            shell_args: Vec::new(),
            working_directory: None,
            scrollback: 1000,
            default_profile: "Default".to_string(),
            tab_bar_height: 30.0,
            padding: 6.0,
            copy_on_select: false,
            middle_click_paste: true,
            notifications: true,
            notify_on_command_finish: false,
            notify_on_bell: false,
            notification_min_seconds: 5.0,
            theme: None,
            theme_variant: ThemeMode::Dark,
        }
    }
}

/// A profile. All fields are optional so named profiles can override only the
/// parts they care about.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub font_family: Option<String>,
    /// Additional font families tried when the primary is unavailable.
    pub font_fallbacks: Option<Vec<String>>,
    pub non_ascii_font_family: Option<String>,
    pub use_non_ascii_font: Option<bool>,
    /// OpenType features such as `liga`, `calt`, or `ss01`.
    pub font_features: Option<Vec<String>>,
    pub use_bold_font: Option<bool>,
    pub use_italic_font: Option<bool>,
    /// Render bold ANSI colors 0–7 with their bright variants.
    pub use_bright_bold: Option<bool>,
    pub font_size: Option<f32>,
    /// Line height multiplier, e.g. 1.25.
    pub line_height: Option<f32>,
    pub foreground: Option<String>,
    pub background: Option<String>,
    pub bold: Option<String>,
    pub cursor: Option<String>,
    pub cursor_text: Option<String>,
    pub selection: Option<String>,
    pub selection_text: Option<String>,
    pub link: Option<String>,
    pub cursor_guide: Option<String>,
    /// Draw a horizontal guide line across the cursor's row.
    pub cursor_guide_enabled: Option<bool>,
    pub badge: Option<String>,
    /// Exactly 16 ANSI colors, or any prefix (remaining colors keep defaults).
    pub palette: Option<Vec<String>>,
    pub cursor_shape: Option<CursorShapeSetting>,
    pub cursor_blink: Option<bool>,
    /// Scrollback lines; `0` means unlimited.
    pub scrollback: Option<usize>,
    pub background_opacity: Option<f32>,
    pub background_image: Option<String>,
    /// Minimum foreground/background contrast, 0.0 (off) to 1.0.
    pub minimum_contrast: Option<f32>,
    pub shell: Option<String>,
    pub working_directory: Option<String>,
}

impl Profile {
    fn apply_to(&self, target: &mut ResolvedProfile) {
        if let Some(v) = &self.font_family {
            target.font_family = Some(v.clone());
        }
        if let Some(v) = &self.font_fallbacks {
            target.font_fallbacks = v.clone();
        }
        if let Some(v) = &self.non_ascii_font_family {
            target.non_ascii_font_family = Some(v.clone());
        }
        if let Some(v) = self.use_non_ascii_font {
            target.use_non_ascii_font = v;
        }
        if let Some(v) = &self.font_features {
            target.font_features = v.clone();
        }
        if let Some(v) = self.use_bold_font {
            target.use_bold_font = v;
        }
        if let Some(v) = self.use_italic_font {
            target.use_italic_font = v;
        }
        if let Some(v) = self.use_bright_bold {
            target.use_bright_bold = v;
        }
        if let Some(v) = self.font_size {
            target.font_size = v.clamp(4.0, 96.0);
        }
        if let Some(v) = self.line_height {
            target.line_height = v.clamp(0.8, 3.0);
        }
        if let Some(v) = self.foreground.as_deref().and_then(Rgb::parse) {
            target.foreground = v;
        }
        if let Some(v) = self.background.as_deref().and_then(Rgb::parse) {
            target.background = v;
        }
        if let Some(v) = self.bold.as_deref().and_then(Rgb::parse) {
            target.bold = v;
        }
        if let Some(v) = self.cursor.as_deref().and_then(Rgb::parse) {
            target.cursor = v;
        }
        if let Some(v) = self.cursor_text.as_deref().and_then(Rgb::parse) {
            target.cursor_text = v;
        }
        if let Some(v) = self.selection.as_deref().and_then(Rgb::parse) {
            target.selection = v;
        }
        if let Some(v) = self.selection_text.as_deref().and_then(Rgb::parse) {
            target.selection_text = v;
        }
        if let Some(v) = self.link.as_deref().and_then(Rgb::parse) {
            target.link = v;
        }
        if let Some(v) = self.cursor_guide.as_deref().and_then(Rgb::parse) {
            target.cursor_guide = v;
        }
        if let Some(v) = self.cursor_guide_enabled {
            target.cursor_guide_enabled = v;
        }
        if let Some(v) = self.badge.as_deref().and_then(Rgb::parse) {
            target.badge = v;
        }
        if let Some(palette) = &self.palette {
            for (slot, value) in target.palette.iter_mut().zip(palette) {
                if let Some(rgb) = Rgb::parse(value) {
                    *slot = rgb;
                }
            }
        }
        if let Some(v) = self.cursor_shape {
            target.cursor_shape = v;
        }
        if let Some(v) = self.cursor_blink {
            target.cursor_blink = v;
        }
        if let Some(v) = self.scrollback {
            target.scrollback = v;
        }
        if let Some(v) = self.background_opacity {
            target.background_opacity = v.clamp(0.0, 1.0);
        }
        if let Some(v) = &self.background_image {
            target.background_image = if v.is_empty() { None } else { Some(v.clone()) };
        }
        if let Some(v) = self.minimum_contrast {
            target.minimum_contrast = v.clamp(0.0, 1.0);
        }
        if let Some(v) = &self.shell {
            target.shell = Some(v.clone());
        }
        if let Some(v) = &self.working_directory {
            target.working_directory = Some(v.clone());
        }
    }
}

/// Fully resolved settings for a profile, with every value concrete.
#[derive(Clone, Debug)]
pub struct ResolvedProfile {
    pub name: String,
    pub font_family: Option<String>,
    pub font_fallbacks: Vec<String>,
    pub non_ascii_font_family: Option<String>,
    pub use_non_ascii_font: bool,
    pub font_features: Vec<String>,
    pub use_bold_font: bool,
    pub use_italic_font: bool,
    pub use_bright_bold: bool,
    pub font_size: f32,
    pub line_height: f32,
    pub foreground: Rgb,
    pub background: Rgb,
    pub bold: Rgb,
    pub cursor: Rgb,
    pub cursor_text: Rgb,
    pub selection: Rgb,
    pub selection_text: Rgb,
    pub link: Rgb,
    pub cursor_guide: Rgb,
    pub cursor_guide_enabled: bool,
    pub badge: Rgb,
    pub palette: [Rgb; 16],
    pub cursor_shape: CursorShapeSetting,
    pub cursor_blink: bool,
    pub scrollback: usize,
    pub background_opacity: f32,
    pub background_image: Option<String>,
    pub minimum_contrast: f32,
    pub shell: Option<String>,
    pub working_directory: Option<String>,
}

impl Default for ResolvedProfile {
    fn default() -> Self {
        // These are iTerm2's Default Bookmark values for dark mode
        // (plists/DefaultBookmark.plist).
        let mut palette = [Rgb::BLACK; 16];
        for (slot, hex) in palette.iter_mut().zip(ITERM2_DEFAULT_PALETTE) {
            *slot = Rgb::parse(hex).expect("built-in palette is valid");
        }
        Self {
            name: "Default".into(),
            font_family: None,
            font_fallbacks: vec![
                "Monaco".into(),
                "JetBrainsMono Nerd Font".into(),
                "Noto Sans Mono CJK JP".into(),
                "DejaVu Sans Mono".into(),
                "Liberation Mono".into(),
            ],
            non_ascii_font_family: None,
            use_non_ascii_font: false,
            font_features: Vec::new(),
            use_bold_font: true,
            use_italic_font: true,
            use_bright_bold: true,
            font_size: 12.0,
            line_height: 1.2,
            foreground: Rgb::parse("#dcdcdc").unwrap(),
            background: Rgb::parse("#15191f").unwrap(),
            bold: Rgb::parse("#ffffff").unwrap(),
            cursor: Rgb::parse("#ffffff").unwrap(),
            cursor_text: Rgb::parse("#000000").unwrap(),
            selection: Rgb::parse("#b3d7ff").unwrap(),
            selection_text: Rgb::parse("#000000").unwrap(),
            link: Rgb::parse("#328eee").unwrap(),
            cursor_guide: Rgb::parse("#60cdf0").unwrap(),
            cursor_guide_enabled: false,
            badge: Rgb::parse("#faa6a6").unwrap(),
            palette,
            cursor_shape: CursorShapeSetting::Block,
            cursor_blink: false,
            scrollback: 1000,
            background_opacity: 1.0,
            background_image: None,
            minimum_contrast: 0.0,
            shell: None,
            working_directory: None,
        }
    }
}

/// Top-level configuration.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    /// Defaults applied to every profile.
    pub profile: Profile,
    /// Named profiles.
    pub profiles: BTreeMap<String, Profile>,
    /// Keybinding overrides, e.g. "ctrl+shift+t" = "new_tab".
    pub keybindings: BTreeMap<String, String>,
    /// Global output triggers, written as `[[triggers]]` tables.
    pub triggers: Vec<Trigger>,
    /// Loaded `.itermcolors` theme (not serialized).
    #[serde(skip)]
    pub theme: Option<ItermColors>,
}

impl Config {
    /// Return the configuration directory (`~/.config/riterm` by default).
    pub fn config_dir() -> PathBuf {
        directories::ProjectDirs::from("org", "riterm", "riterm")
            .map(|p| p.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".").join(".riterm"))
    }

    pub fn default_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    /// Load from `path`, creating a commented default config if it is missing.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::write(path, DEFAULT_CONFIG)
                .with_context(|| format!("writing default config {}", path.display()))?;
        }

        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let mut config: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;

        if let Some(theme) = config.general.theme.clone() {
            if let Some(preset) = presets::preset(&theme) {
                config.theme = Some(ItermColors::from_pairs(preset.colors));
            } else {
                let theme_path = expand_home(&theme);
                match parse_itermcolors(&theme_path) {
                    Ok(theme) => config.theme = Some(theme),
                    Err(err) => {
                        tracing::warn!(
                            "could not load theme {theme:?} ({}): {err:#}; built-in names: {}",
                            theme_path.display(),
                            presets::names().collect::<Vec<_>>().join(", ")
                        )
                    }
                }
            }
        }

        Ok(config)
    }

    /// Save the configuration to `path`.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self).context("serializing config")?;
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// Select a built-in preset by name or load an `.itermcolors` file.
    pub fn set_theme(&mut self, name_or_path: &str) -> Result<()> {
        if let Some(preset) = presets::preset(name_or_path) {
            self.theme = Some(ItermColors::from_pairs(preset.colors));
        } else {
            let path = expand_home(name_or_path);
            self.theme = Some(parse_itermcolors(&path)?);
        }
        self.general.theme = Some(name_or_path.to_string());
        Ok(())
    }

    /// Resolve a named profile against the defaults and optional theme.
    pub fn resolve(&self, name: &str) -> ResolvedProfile {
        let mut resolved = ResolvedProfile::default();

        if let Some(theme) = &self.theme {
            theme.apply_to(&mut resolved, self.general.theme_variant.resolved());
        }
        resolved.scrollback = self.general.scrollback;
        self.profile.apply_to(&mut resolved);
        if let Some(profile) = self.profiles.get(name) {
            profile.apply_to(&mut resolved);
        }

        resolved.name = name.to_string();
        if resolved.shell.is_none() {
            resolved.shell = self.general.shell.clone();
        }
        if resolved.working_directory.is_none() {
            resolved.working_directory = self.general.working_directory.clone();
        }
        resolved
    }

    /// Built-in theme names accepted by `[general].theme`.
    pub fn theme_names() -> impl Iterator<Item = &'static str> {
        presets::names()
    }

    pub fn profile_names(&self) -> Vec<String> {
        let mut names = vec!["Default".to_string()];
        names.extend(self.profiles.keys().cloned());
        names
    }
}

pub fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = directories::BaseDirs::new() {
            return home.home_dir().join(rest);
        }
    }
    PathBuf::from(path)
}

const DEFAULT_CONFIG: &str = r##"# Riterm configuration
#
# Colors accept "#rrggbb". Profiles inherit the values in [profile] and only
# override what they set. riterm ships every built-in iTerm2 color preset;
# list them with:
#   riterm --list-themes
# To use one:
#   theme = "Solarized Dark"
# Or import an iTerm2 file:
#   riterm --import-theme ~/Downloads/Solarized.itermcolors
# To import an iTerm2 profile:
#   riterm --import-iterm2 ~/Library/Preferences/com.googlecode.iterm2.plist

[general]
default_profile = "Default"
scrollback = 1000
tab_bar_height = 30.0
padding = 6.0
copy_on_select = false
middle_click_paste = true
notifications = true
notify_on_command_finish = false
notify_on_bell = false
notification_min_seconds = 5.0
theme_variant = "dark"
# theme = "Smoooooth"
# shell = "/bin/zsh"
# shell_args = ["-l"]
# working_directory = "~"

[profile]
font_size = 12.0
line_height = 1.2
# font_family = "JetBrainsMono Nerd Font"
# font_fallbacks = ["Noto Sans Mono CJK JP", "DejaVu Sans Mono"]
# non_ascii_font_family = "Noto Sans Mono CJK JP"
# font_features = ["liga", "calt"]
# foreground = "#dcdcdc"
# background = "#15191f"
# bold = "#ffffff"
# cursor = "#ffffff"
# cursor_text = "#000000"
# cursor_guide = "#60cdf0"
# cursor_guide_enabled = false
# selection = "#b3d7ff"
# selection_text = "#000000"
# link = "#328eee"
# cursor_shape = "block"
# cursor_blink = false
# minimum_contrast = 0.0

[profiles.Work]
font_size = 14.0

[keybindings]
# "ctrl+shift+t" = "new_tab"

# Output triggers run on every completed line of terminal output.
# action = "notify" posts a desktop notification; "bell" marks the tab.
# [[triggers]]
# regex = "(?i)\\b(error|failed|panicked)\\b"
# action = "notify"
# text = "Command reported an error"
# enabled = true
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_is_valid() {
        let cfg = Config::default();
        let p = cfg.resolve("Default");
        assert_eq!(p.palette.len(), 16);
        assert!(p.font_size >= 4.0);
        assert_eq!(p.background, Rgb::parse("#15191f").unwrap());
    }

    #[test]
    fn default_config_text_parses() {
        let config: Config = toml::from_str(DEFAULT_CONFIG).expect("default config parses");
        assert!(config.general.notifications);
        assert!(!config.general.notify_on_command_finish);
        assert_eq!(config.general.notification_min_seconds, 5.0);
        assert!(config.triggers.is_empty());
    }

    #[test]
    fn triggers_deserialize_with_defaults() {
        let config: Config = toml::from_str(
            r#"
[[triggers]]
regex = "(?i)\\b(error|failed)\\b"
action = "bell"
text = "Build failed"
"#,
        )
        .expect("trigger config parses");
        assert_eq!(config.triggers.len(), 1);
        let trigger = &config.triggers[0];
        assert_eq!(trigger.action, TriggerAction::Bell);
        assert_eq!(trigger.text.as_deref(), Some("Build failed"));
        assert!(trigger.enabled);
    }

    #[test]
    fn profile_overrides_merge() {
        let mut cfg = Config::default();
        cfg.profiles.insert(
            "Test".into(),
            Profile {
                background: Some("#112233".into()),
                ..Profile::default()
            },
        );
        let p = cfg.resolve("Test");
        assert_eq!(p.background, Rgb::parse("#112233").unwrap());
        assert_eq!(p.foreground, Rgb::parse("#dcdcdc").unwrap());
    }

    #[test]
    fn builtin_theme_resolves_by_name() {
        let mut cfg = Config::default();
        cfg.general.theme = Some("Solarized Dark".into());
        let preset = presets::preset("Solarized Dark").unwrap();
        cfg.theme = Some(ItermColors::from_pairs(preset.colors));
        let p = cfg.resolve("Default");
        assert_eq!(p.background, Rgb::parse("#002b36").unwrap());
        assert_eq!(p.palette[1], Rgb::parse("#dc322f").unwrap());
    }

    #[test]
    fn auto_theme_variant_uses_colorfgbg() {
        assert_eq!(
            ThemeMode::Auto.resolve_from(None, Some("0;15")),
            ThemeVariant::Light
        );
        assert_eq!(
            ThemeMode::Auto.resolve_from(None, Some("15;0")),
            ThemeVariant::Dark
        );
        assert_eq!(
            ThemeMode::Auto.resolve_from(Some("light"), None),
            ThemeVariant::Light
        );
    }
}
