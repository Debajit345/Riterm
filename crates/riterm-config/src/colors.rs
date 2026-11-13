use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// An 8-bit sRGB color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Self = Self { r: 0, g: 0, b: 0 };

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::from_str(value).ok()
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Linear-ish RGBA in 0..1 for GPU shaders.
    pub fn to_rgba_f32(self, alpha: f32) -> [f32; 4] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            alpha,
        ]
    }

    pub fn mix(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Self {
            r: mix(self.r, other.r),
            g: mix(self.g, other.g),
            b: mix(self.b, other.b),
        }
    }
}

impl FromStr for Rgb {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let hex = s
            .strip_prefix('#')
            .or_else(|| s.strip_prefix("0x"))
            .unwrap_or(s);
        let expanded;
        let hex = if hex.len() == 3 {
            expanded = hex.chars().flat_map(|c| [c, c]).collect::<String>();
            expanded.as_str()
        } else {
            hex
        };
        if hex.len() != 6 {
            return Err(format!("invalid color {s:?}"));
        }
        let r = u8::from_str_radix(&hex[0..2], 16).map_err(|_| format!("invalid color {s:?}"))?;
        let g = u8::from_str_radix(&hex[2..4], 16).map_err(|_| format!("invalid color {s:?}"))?;
        let b = u8::from_str_radix(&hex[4..6], 16).map_err(|_| format!("invalid color {s:?}"))?;
        Ok(Self { r, g, b })
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// Convert a `.itermcolors` component (0.0..1.0 float) to u8.
pub(crate) fn component_to_u8(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex() {
        assert_eq!(Rgb::parse("#1a2b3c"), Some(Rgb::new(0x1a, 0x2b, 0x3c)));
        assert_eq!(Rgb::parse("abc"), Some(Rgb::new(0xaa, 0xbb, 0xcc)));
        assert_eq!(Rgb::parse("#12345"), None);
    }
}
