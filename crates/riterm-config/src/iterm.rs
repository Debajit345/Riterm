//! Importers for iTerm2 color presets and profiles.

use std::path::Path;

use anyhow::{Context, Result};
use plist::Value;
use serde::{Deserialize, Serialize};

use super::colors::component_to_u8;
use super::{Profile, ResolvedProfile, Rgb};

#[cfg(test)]
use super::presets;

/// Which appearance variant of a preset to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeVariant {
    Dark,
    Light,
}

/// User-facing appearance selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
    Auto,
}

impl ThemeMode {
    /// Resolve `Auto` from the environment. iTerm2 follows the macOS system
    /// appearance; on Linux we honour `RITERM_THEME_VARIANT` and the classic
    /// `COLORFGBG` convention, falling back to dark.
    pub fn resolved(self) -> ThemeVariant {
        let variant = std::env::var("RITERM_THEME_VARIANT").ok();
        let colorfgbg = std::env::var("COLORFGBG").ok();
        self.resolve_from(variant.as_deref(), colorfgbg.as_deref())
    }

    /// Pure form of [`Self::resolved`], used by tests.
    pub fn resolve_from(self, variant: Option<&str>, colorfgbg: Option<&str>) -> ThemeVariant {
        match self {
            Self::Dark => ThemeVariant::Dark,
            Self::Light => ThemeVariant::Light,
            Self::Auto => {
                if let Some(value) = variant {
                    if value.eq_ignore_ascii_case("light") {
                        return ThemeVariant::Light;
                    }
                    if value.eq_ignore_ascii_case("dark") {
                        return ThemeVariant::Dark;
                    }
                }
                if let Some(value) = colorfgbg {
                    if let Some(background) = value.rsplit(';').next() {
                        if background
                            .parse::<u8>()
                            .map(|index| index >= 7)
                            .unwrap_or(false)
                        {
                            return ThemeVariant::Light;
                        }
                    }
                }
                ThemeVariant::Dark
            }
        }
    }
}

/// One resolved set of colors from an iTerm2 preset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColorSet {
    pub foreground: Option<Rgb>,
    pub background: Option<Rgb>,
    pub bold: Option<Rgb>,
    pub cursor: Option<Rgb>,
    pub cursor_text: Option<Rgb>,
    pub selection: Option<Rgb>,
    pub selected_text: Option<Rgb>,
    pub link: Option<Rgb>,
    pub cursor_guide: Option<Rgb>,
    pub badge: Option<Rgb>,
    pub palette: [Option<Rgb>; 16],
}

impl ColorSet {
    fn set_key(&mut self, key: &str, rgb: Rgb) -> bool {
        match key {
            "Foreground Color" => self.foreground = Some(rgb),
            "Background Color" => self.background = Some(rgb),
            "Bold Color" => self.bold = Some(rgb),
            "Cursor Color" => self.cursor = Some(rgb),
            "Cursor Text Color" => self.cursor_text = Some(rgb),
            "Selection Color" => self.selection = Some(rgb),
            "Selected Text Color" => self.selected_text = Some(rgb),
            "Link Color" => self.link = Some(rgb),
            "Cursor Guide Color" => self.cursor_guide = Some(rgb),
            "Badge Color" => self.badge = Some(rgb),
            _ => {
                let Some(index) = key
                    .strip_prefix("Ansi ")
                    .and_then(|rest| rest.strip_suffix(" Color"))
                    .and_then(|index| index.parse::<usize>().ok())
                else {
                    return false;
                };
                if index >= self.palette.len() {
                    return false;
                }
                self.palette[index] = Some(rgb);
            }
        }
        true
    }

    fn apply_to(&self, target: &mut ResolvedProfile) {
        if let Some(value) = self.foreground {
            target.foreground = value;
        }
        if let Some(value) = self.background {
            target.background = value;
        }
        if let Some(value) = self.bold {
            target.bold = value;
        }
        if let Some(value) = self.cursor {
            target.cursor = value;
        }
        if let Some(value) = self.cursor_text {
            target.cursor_text = value;
        }
        if let Some(value) = self.selection {
            target.selection = value;
        }
        if let Some(value) = self.selected_text {
            target.selection_text = value;
        }
        if let Some(value) = self.link {
            target.link = value;
        }
        if let Some(value) = self.cursor_guide {
            target.cursor_guide = value;
        }
        if let Some(value) = self.badge {
            target.badge = value;
        }
        for (slot, value) in target.palette.iter_mut().zip(self.palette.iter()) {
            if let Some(value) = value {
                *slot = *value;
            }
        }
    }

    fn to_profile(&self) -> Profile {
        let mut palette = Vec::with_capacity(16);
        let mut all_palette = true;
        for color in &self.palette {
            match color {
                Some(color) => palette.push(color.to_hex()),
                None => {
                    all_palette = false;
                    break;
                }
            }
        }

        Profile {
            foreground: self.foreground.map(|color| color.to_hex()),
            background: self.background.map(|color| color.to_hex()),
            bold: self.bold.map(|color| color.to_hex()),
            cursor: self.cursor.map(|color| color.to_hex()),
            cursor_text: self.cursor_text.map(|color| color.to_hex()),
            selection: self.selection.map(|color| color.to_hex()),
            selection_text: self.selected_text.map(|color| color.to_hex()),
            link: self.link.map(|color| color.to_hex()),
            cursor_guide: self.cursor_guide.map(|color| color.to_hex()),
            badge: self.badge.map(|color| color.to_hex()),
            palette: all_palette.then_some(palette),
            ..Profile::default()
        }
    }
}

/// An iTerm2 color preset, including optional light/dark color sets.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItermColors {
    pub base: ColorSet,
    pub dark: Option<ColorSet>,
    pub light: Option<ColorSet>,
}

impl ItermColors {
    /// Parse the compact pair list used by [`presets`].
    pub fn from_pairs(pairs: &[(&str, Rgb)]) -> Self {
        let mut colors = Self::default();
        for (key, rgb) in pairs {
            colors.set_key(key, *rgb);
        }
        colors
    }

    /// Parse an iTerm2 color dictionary.
    pub fn from_dict(dict: &plist::Dictionary) -> Self {
        let mut colors = Self::default();
        for (key, value) in dict {
            let Some(color) = value.as_dictionary() else {
                continue;
            };
            let Some(rgb) = dict_color(color) else {
                continue;
            };
            colors.set_key(key, rgb);
        }
        colors
    }

    fn set_key(&mut self, key: &str, rgb: Rgb) {
        let (key, variant) = if let Some(key) = key.strip_suffix(" (Dark)") {
            (key, Some(ThemeVariant::Dark))
        } else if let Some(key) = key.strip_suffix(" (Light)") {
            (key, Some(ThemeVariant::Light))
        } else {
            (key, None)
        };
        let set = match variant {
            Some(ThemeVariant::Dark) => self.dark.get_or_insert_with(ColorSet::default),
            Some(ThemeVariant::Light) => self.light.get_or_insert_with(ColorSet::default),
            None => &mut self.base,
        };
        set.set_key(key, rgb);
    }

    /// The most specific color set for an appearance, falling back to the base.
    pub fn for_variant(&self, variant: ThemeVariant) -> &ColorSet {
        match variant {
            ThemeVariant::Dark => self.dark.as_ref().unwrap_or(&self.base),
            ThemeVariant::Light => self.light.as_ref().unwrap_or(&self.base),
        }
    }

    pub fn apply_to(&self, target: &mut ResolvedProfile, variant: ThemeVariant) {
        self.for_variant(variant).apply_to(target);
    }

    /// Convert the base colors to profile fields (used by `--import-theme`).
    pub fn to_profile(&self) -> Profile {
        self.base.to_profile()
    }

    pub fn is_empty(&self) -> bool {
        self.base == ColorSet::default() && self.dark.is_none() && self.light.is_none()
    }
}

/// A profile imported from iTerm2's `com.googlecode.iterm2.plist`.
#[derive(Clone, Debug)]
pub struct ImportedProfile {
    pub name: String,
    pub profile: Profile,
}

/// Parse an iTerm2 `.itermcolors` file.
pub fn parse_itermcolors(path: &Path) -> Result<ItermColors> {
    let value = Value::from_file(path)
        .with_context(|| format!("reading color preset {}", path.display()))?;
    let dict = value
        .as_dictionary()
        .ok_or_else(|| anyhow::anyhow!("{} is not a dictionary plist", path.display()))?;

    let colors = ItermColors::from_dict(dict);
    if colors.is_empty() {
        anyhow::bail!("{} contains no iTerm2 colors", path.display());
    }
    Ok(colors)
}

/// Parse either an iTerm2 preferences plist (`New Bookmarks`) or a single
/// profile dictionary.
pub fn import_iterm2_prefs(path: &Path) -> Result<Vec<ImportedProfile>> {
    let value = Value::from_file(path)
        .with_context(|| format!("reading iTerm2 preferences {}", path.display()))?;
    let dict = value
        .as_dictionary()
        .ok_or_else(|| anyhow::anyhow!("{} is not a dictionary plist", path.display()))?;

    if let Some(bookmarks) = dict.get("New Bookmarks").and_then(Value::as_array) {
        let mut profiles = Vec::new();
        for (index, bookmark) in bookmarks.iter().enumerate() {
            let Some(bookmark) = bookmark.as_dictionary() else {
                continue;
            };
            let name = bookmark
                .get("Name")
                .and_then(Value::as_string)
                .map(str::to_string)
                .unwrap_or_else(|| format!("Imported {index}"));
            profiles.push(ImportedProfile {
                name,
                profile: profile_from_dict(bookmark),
            });
        }
        if !profiles.is_empty() {
            return Ok(profiles);
        }
    }

    // Fall back to a single profile dictionary.
    Ok(vec![ImportedProfile {
        name: dict
            .get("Name")
            .and_then(Value::as_string)
            .unwrap_or("Imported")
            .to_string(),
        profile: profile_from_dict(dict),
    }])
}

fn profile_from_dict(dict: &plist::Dictionary) -> Profile {
    let font = dict.get("Normal Font").and_then(Value::as_string);
    let (font_family, font_size) = font
        .and_then(parse_iterm_font)
        .map_or((None, None), |(family, size)| (Some(family), Some(size)));

    let non_ascii_font = dict.get("Non Ascii Font").and_then(Value::as_string);
    let (non_ascii_font_family, _) = non_ascii_font
        .and_then(parse_iterm_font)
        .map_or((None, None), |(family, size)| (Some(family), Some(size)));

    let mut palette = Vec::with_capacity(16);
    let mut all_palette = true;
    for i in 0..16 {
        match dict_color_value(dict, &format!("Ansi {i} Color")) {
            Some(color) => palette.push(color.to_hex()),
            None => {
                all_palette = false;
                break;
            }
        }
    }

    Profile {
        font_family,
        font_size,
        non_ascii_font_family,
        use_non_ascii_font: dict_bool(dict, "Use Non-ASCII Font"),
        use_bold_font: dict_bool(dict, "Use Bold Font"),
        use_italic_font: dict_bool(dict, "Use Italic Font"),
        use_bright_bold: dict_bool(dict, "Use Bright Bold"),
        line_height: None,
        foreground: dict_color_value(dict, "Foreground Color").map(|c| c.to_hex()),
        background: dict_color_value(dict, "Background Color").map(|c| c.to_hex()),
        bold: dict_color_value(dict, "Bold Color").map(|c| c.to_hex()),
        cursor: dict_color_value(dict, "Cursor Color").map(|c| c.to_hex()),
        cursor_text: dict_color_value(dict, "Cursor Text Color").map(|c| c.to_hex()),
        selection: dict_color_value(dict, "Selection Color").map(|c| c.to_hex()),
        selection_text: dict_color_value(dict, "Selected Text Color").map(|c| c.to_hex()),
        link: dict_color_value(dict, "Link Color").map(|c| c.to_hex()),
        cursor_guide: dict_color_value(dict, "Cursor Guide Color").map(|c| c.to_hex()),
        badge: dict_color_value(dict, "Badge Color").map(|c| c.to_hex()),
        palette: all_palette.then_some(palette),
        cursor_blink: dict_bool(dict, "Blinking Cursor"),
        cursor_shape: dict_cursor_shape(dict),
        scrollback: dict_scrollback(dict),
        working_directory: dict
            .get("Working Directory")
            .and_then(Value::as_string)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        shell: None,
        ..Profile::default()
    }
}

fn dict_bool(dict: &plist::Dictionary, key: &str) -> Option<bool> {
    dict.get(key).and_then(Value::as_boolean)
}

fn dict_scrollback(dict: &plist::Dictionary) -> Option<usize> {
    if dict_bool(dict, "Unlimited Scrollback") == Some(true) {
        return Some(0);
    }
    dict.get("Scrollback Lines")
        .and_then(Value::as_unsigned_integer)
        .map(|lines| lines as usize)
}

fn dict_cursor_shape(dict: &plist::Dictionary) -> Option<super::CursorShapeSetting> {
    let value = dict
        .get("Cursor Type")
        .and_then(Value::as_unsigned_integer)?;
    Some(match value {
        1 => super::CursorShapeSetting::Underline,
        2 => super::CursorShapeSetting::Beam,
        _ => super::CursorShapeSetting::Block,
    })
}

/// iTerm2 stores fonts as e.g. `"MesloLGS-NF-Regular 14"`.
fn parse_iterm_font(value: &str) -> Option<(String, f32)> {
    let (family, size) = value.rsplit_once(' ')?;
    let size = size.parse::<f32>().ok()?;
    let family = family
        .trim_end_matches("-Regular")
        .trim_end_matches("-BoldItalic")
        .trim_end_matches("-Bold")
        .trim_end_matches("-Italic")
        .trim_end_matches("-Light")
        .trim_end_matches("-Medium")
        .replace('-', " ");
    Some((family, size))
}

fn dict_color_value(dict: &plist::Dictionary, key: &str) -> Option<Rgb> {
    dict_color(dict.get(key)?.as_dictionary()?)
}

fn dict_color(color: &plist::Dictionary) -> Option<Rgb> {
    let component = |name: &str| -> Option<f64> {
        let value = color.get(name)?;
        value
            .as_real()
            .or_else(|| value.as_string().and_then(|text| text.parse::<f64>().ok()))
    };
    let r = component("Red Component").or_else(|| component("Red Color Component"))?;
    let g = component("Green Component").or_else(|| component("Green Color Component"))?;
    let b = component("Blue Component").or_else(|| component("Blue Color Component"))?;
    Some(Rgb::new(
        component_to_u8(r),
        component_to_u8(g),
        component_to_u8(b),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_font_names() {
        assert_eq!(
            parse_iterm_font("MesloLGS-NF-Regular 14"),
            Some(("MesloLGS NF".into(), 14.0))
        );
        assert_eq!(
            parse_iterm_font("JetBrainsMonoNF-Bold 12.5"),
            Some(("JetBrainsMonoNF".into(), 12.5))
        );
    }

    #[test]
    fn builtin_solarized_dark_matches_iterm2() {
        let preset = presets::preset("Solarized Dark").expect("bundled preset");
        let colors = ItermColors::from_pairs(preset.colors);
        let set = colors.for_variant(ThemeVariant::Dark);
        assert_eq!(set.background, Rgb::parse("#002b36"));
        assert_eq!(set.foreground, Rgb::parse("#839496"));
        assert_eq!(set.palette[1], Rgb::parse("#dc322f"));
        assert_eq!(set.palette[15], Rgb::parse("#fdf6e3"));
    }

    #[test]
    fn dark_and_light_variants_are_separate() {
        let preset = presets::preset("Solarized").expect("bundled preset");
        let colors = ItermColors::from_pairs(preset.colors);
        assert_eq!(
            colors
                .for_variant(ThemeVariant::Dark)
                .background
                .and_then(|c| Some(c.to_hex())),
            Some("#002b36".to_string())
        );
        assert_eq!(
            colors
                .for_variant(ThemeVariant::Light)
                .background
                .map(|c| c.to_hex()),
            Some("#fdf6e3".to_string())
        );
    }
}
