// SPDX-License-Identifier: GPL-3.0-or-later
//! Color models, include/exclude rules and the compiled lookup table (PLAN §5.4).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub mod hsv;
pub mod lut;
pub mod oklab;
pub mod rule;

pub use rule::{ColorEntry, ColorModel, ColorRules, CompiledRules, ShadePreset, Tolerances};

/// A 24-bit sRGB color, `0xRRGGBB`. Profiles and the protocol write it as `"#RRGGBB"`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgb(u32);

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self((r as u32) << 16 | (g as u32) << 8 | b as u32)
    }

    /// Keeps the low 24 bits of `v`.
    pub const fn from_u32(v: u32) -> Self {
        Self(v & 0x00FF_FFFF)
    }

    pub const fn to_u32(self) -> u32 {
        self.0
    }

    pub const fn r(self) -> u8 {
        (self.0 >> 16) as u8
    }

    pub const fn g(self) -> u8 {
        (self.0 >> 8) as u8
    }

    pub const fn b(self) -> u8 {
        self.0 as u8
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:06X}", self.0)
    }
}

impl fmt::Debug for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("expected a color like #3BD16F, got {0:?}")]
pub struct ParseRgbError(String);

impl FromStr for Rgb {
    type Err = ParseRgbError;

    /// Accepts `#RRGGBB` or `RRGGBB`, in either case.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ParseRgbError(s.to_owned()));
        }
        u32::from_str_radix(hex, 16)
            .map(Self)
            .map_err(|_| ParseRgbError(s.to_owned()))
    }
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// What the lookup table records for one color (2 bits, PLAN §5.4).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Class {
    /// In no include range.
    Absent = 0,
    /// In an include color's wide range only: enough to count as "still there".
    Wide = 1,
    /// In an include color's tight range: enough to start an appearance.
    Tight = 2,
    /// In an exclude range. Exclusion always wins.
    Excluded = 3,
}

impl Class {
    pub const fn from_bits(bits: u8) -> Self {
        match bits & 3 {
            0 => Self::Absent,
            1 => Self::Wide,
            2 => Self::Tight,
            _ => Self::Excluded,
        }
    }

    pub const fn bits(self) -> u8 {
        self as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_parses_and_prints_hex() {
        let c: Rgb = "#3BD16F".parse().unwrap();
        assert_eq!(c, Rgb::new(0x3B, 0xD1, 0x6F));
        assert_eq!((c.r(), c.g(), c.b()), (0x3B, 0xD1, 0x6F));
        assert_eq!(c.to_string(), "#3BD16F");
        assert_eq!("3bd16f".parse::<Rgb>().unwrap(), c);
        for bad in [
            "", "#", "#3BD16", "#3BD16F0", "#GGGGGG", "+3BD16", "#+3BD16", " #3BD16F", "#3BD 6F",
        ] {
            assert!(bad.parse::<Rgb>().is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn rgb_keeps_24_bits() {
        assert_eq!(Rgb::from_u32(0xFF12_3456).to_u32(), 0x12_3456);
    }

    #[test]
    fn rgb_serializes_as_a_hex_string() {
        let c = Rgb::new(0x3B, 0xD1, 0x6F);
        assert_eq!(serde_json::to_string(&c).unwrap(), "\"#3BD16F\"");
        assert_eq!(serde_json::from_str::<Rgb>("\"#3bd16f\"").unwrap(), c);
        assert!(serde_json::from_str::<Rgb>("\"green\"").is_err());
    }

    #[test]
    fn class_bits_round_trip() {
        for class in [Class::Absent, Class::Wide, Class::Tight, Class::Excluded] {
            assert_eq!(Class::from_bits(class.bits()), class);
        }
    }
}
