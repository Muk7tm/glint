// SPDX-License-Identifier: MIT
//! The compiled lookup table: 2 bits for each of the 16,777,216 24-bit colors, 4 MiB in all
//! (PLAN §5.4).
//!
//! [`Lut::build`] runs in parallel on whatever rayon pool the caller installs (the engine uses a
//! low-priority pool on the controller thread) and writes every page, so the finished table is
//! prefaulted. The hot loop only reads it: one shift, one mask and one load per pixel.

use std::fmt;

use rayon::prelude::*;

use super::rule::CompiledRules;
use super::{Class, Rgb};

/// The number of 24-bit colors.
pub const COLORS: usize = 1 << 24;

/// Colors per `u64` word, at 2 bits each.
const PER_WORD: usize = 32;
const WORDS: usize = COLORS / PER_WORD;

/// Words per parallel task (32,768 colors).
const CHUNK_WORDS: usize = 1024;

/// How many colors fall in each class. The UI uses these for its warnings.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct LutStats {
    pub tight: u32,
    /// In an include color's wide range but in no tight range.
    pub wide: u32,
    pub excluded: u32,
    /// Excluded colors that an include range would otherwise have matched. Above 0, an
    /// exclusion cuts into an include range (§5.4).
    pub overlap: u32,
}

impl LutStats {
    fn add(&mut self, class: Class, overlap: bool) {
        match class {
            Class::Absent => {}
            Class::Wide => self.wide += 1,
            Class::Tight => self.tight += 1,
            Class::Excluded => self.excluded += 1,
        }
        self.overlap += u32::from(overlap);
    }

    fn merge(self, other: Self) -> Self {
        Self {
            tight: self.tight + other.tight,
            wide: self.wide + other.wide,
            excluded: self.excluded + other.excluded,
            overlap: self.overlap + other.overlap,
        }
    }
}

/// The class of every 24-bit color.
pub struct Lut {
    words: Box<[u64; WORDS]>,
    stats: LutStats,
}

impl Lut {
    pub fn build(rules: &CompiledRules) -> Self {
        let mut words: Box<[u64; WORDS]> = vec![0u64; WORDS]
            .into_boxed_slice()
            .try_into()
            .expect("the vector has exactly WORDS entries");
        let stats = words
            .par_chunks_mut(CHUNK_WORDS)
            .enumerate()
            .map(|(chunk, out)| {
                let mut stats = LutStats::default();
                let first = chunk * CHUNK_WORDS * PER_WORD;
                for (i, word) in out.iter_mut().enumerate() {
                    let base = first + i * PER_WORD;
                    let mut packed = 0u64;
                    for j in 0..PER_WORD {
                        let (class, overlap) = rules.evaluate(Rgb::from_u32((base + j) as u32));
                        stats.add(class, overlap);
                        packed |= u64::from(class.bits()) << (2 * j);
                    }
                    *word = packed;
                }
                stats
            })
            .reduce(LutStats::default, LutStats::merge);
        Self { words, stats }
    }

    /// The 2-bit class code of `rgb` (`0xRRGGBB`; higher bits are ignored).
    #[inline]
    pub fn bits(&self, rgb: u32) -> u8 {
        let c = (rgb & 0x00FF_FFFF) as usize;
        ((self.words[c / PER_WORD] >> (2 * (c % PER_WORD))) & 3) as u8
    }

    #[inline]
    pub fn class(&self, rgb: u32) -> Class {
        Class::from_bits(self.bits(rgb))
    }

    pub fn stats(&self) -> LutStats {
        self.stats
    }
}

impl fmt::Debug for Lut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lut")
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::super::rule::{ColorEntry, ColorModel, ColorRules, ShadePreset};
    use super::*;

    fn rgb(s: &str) -> Rgb {
        s.parse().unwrap()
    }

    /// A plain reading of PLAN §5.4 and ADR 0005, written separately from `CompiledRules`: it
    /// converts both colors on every call, evaluates every range eagerly, and applies the
    /// precedence (excluded, then tight, then wide) as written.
    mod reference {
        use super::super::super::hsv::{HSV_ACHROMATIC_CHROMA, Hsv};
        use super::super::super::oklab::{ACHROMATIC_CHROMA, Oklch, hue_distance};
        use super::super::super::rule::{ColorEntry, ColorRules, EXCLUDE_PRESET, INCLUDE_PRESET, Tolerances};
        use super::super::super::{Class, Rgb};

        pub fn evaluate(rules: &ColorRules, px: Rgb) -> (Class, bool) {
            let margin = rules.exit_margin.max(1.0);
            let any = |entries: &[ColorEntry], preset, scale| {
                entries
                    .iter()
                    .any(|e| inside(e.hex, e.tolerances(rules.model, preset), scale, px))
            };
            let tight = any(&rules.include, INCLUDE_PRESET, 1.0);
            let wide = any(&rules.include, INCLUDE_PRESET, margin);
            if any(&rules.exclude, EXCLUDE_PRESET, 1.0) {
                (Class::Excluded, tight || wide)
            } else if tight {
                (Class::Tight, false)
            } else if wide {
                (Class::Wide, false)
            } else {
                (Class::Absent, false)
            }
        }

        fn inside(target: Rgb, tol: Tolerances, scale: f64, px: Rgb) -> bool {
            match tol {
                Tolerances::Oklch { l, c, h } => {
                    let (t, p) = (Oklch::from_rgb(target), Oklch::from_rgb(px));
                    let hue_ok = if t.c < ACHROMATIC_CHROMA {
                        true // gray target: hue ignored
                    } else {
                        p.c >= ACHROMATIC_CHROMA && hue_distance(p.h, t.h) <= h * scale
                    };
                    (p.l - t.l).abs() <= l * scale && (p.c - t.c).abs() <= c * scale && hue_ok
                }
                Tolerances::Hsv { h, s, v } => {
                    let (t, p) = (Hsv::from_rgb(target), Hsv::from_rgb(px));
                    let hue_ok = if t.chroma < HSV_ACHROMATIC_CHROMA {
                        true
                    } else {
                        p.chroma >= HSV_ACHROMATIC_CHROMA && hue_distance(p.h, t.h) <= h * scale
                    };
                    (p.s - t.s).abs() <= s * scale && (p.v - t.v).abs() <= v * scale && hue_ok
                }
                Tolerances::Rgb { t } => {
                    let near = |a: u8, b: u8| f64::from(a.abs_diff(b)) <= t * scale;
                    near(px.r(), target.r()) && near(px.g(), target.g()) && near(px.b(), target.b())
                }
            }
        }
    }

    fn entry(hex: &str, f: impl FnOnce(&mut ColorEntry)) -> ColorEntry {
        let mut e = ColorEntry::new(rgb(hex));
        f(&mut e);
        e
    }

    fn cases() -> Vec<(&'static str, ColorRules)> {
        use ColorModel::{Hsv, Oklch, Rgb};
        let preset = |p: ShadePreset, m, hex| p.entry(m, rgb(hex));
        vec![
            (
                "appendix B",
                ColorRules {
                    model: Oklch,
                    exit_margin: 1.5,
                    include: vec![preset(ShadePreset::Normal, Oklch, "#3BD16F")],
                    exclude: vec![preset(ShadePreset::Tight, Oklch, "#6BD13B")],
                },
            ),
            (
                "gray and red with default tolerances, an exclusion inside an include range",
                ColorRules {
                    model: Oklch,
                    exit_margin: 2.0,
                    include: vec![
                        ColorEntry::new(rgb("#808080")),
                        preset(ShadePreset::Loose, Oklch, "#E03030"),
                    ],
                    exclude: vec![ColorEntry::new(rgb("#E83838"))],
                },
            ),
            (
                "a muted target near the gray guard, a hue that wraps past 0°",
                ColorRules {
                    model: Oklch,
                    exit_margin: 1.5,
                    include: vec![
                        ColorEntry::new(rgb("#7A8A7E")),
                        entry("#E03078", |e| e.tol_h = Some(4.0)),
                    ],
                    exclude: vec![],
                },
            ),
            (
                "the widest legal ranges",
                ColorRules {
                    model: Oklch,
                    exit_margin: 3.0,
                    include: vec![entry("#3BD16F", |e| {
                        e.tol_l = Some(1.0);
                        e.tol_c = Some(0.5);
                        e.tol_h = Some(180.0);
                    })],
                    exclude: vec![entry("#000000", |e| e.tol_l = Some(0.2))],
                },
            ),
            (
                "hsv",
                ColorRules {
                    model: Hsv,
                    exit_margin: 1.5,
                    include: vec![
                        ColorEntry::new(rgb("#3BD16F")),
                        ColorEntry::new(rgb("#808080")),
                        preset(ShadePreset::Loose, Hsv, "#FF0A0A"),
                    ],
                    exclude: vec![ColorEntry::new(rgb("#6BD13B"))],
                },
            ),
            (
                "rgb, with an exact-color hole",
                ColorRules {
                    model: Rgb,
                    exit_margin: 1.5,
                    include: vec![
                        entry("#3BD16F", |e| e.tol_rgb = Some(12)),
                        entry("#000000", |e| e.tol_rgb = Some(0)),
                    ],
                    exclude: vec![entry("#3BD16F", |e| e.tol_rgb = Some(0))],
                },
            ),
            (
                "exclusions only",
                ColorRules {
                    exclude: vec![ColorEntry::new(rgb("#3BD16F"))],
                    ..ColorRules::default()
                },
            ),
            ("empty", ColorRules::default()),
        ]
    }

    #[test]
    fn every_color_matches_the_reference_math() {
        let mut all = Vec::new();
        for (name, rules) in cases() {
            let lut = Lut::build(&CompiledRules::new(&rules));
            let (mismatches, stats) = (0..COLORS as u32)
                .into_par_iter()
                .map(|c| {
                    let (class, overlap) = reference::evaluate(&rules, Rgb::from_u32(c));
                    let mut stats = LutStats::default();
                    stats.add(class, overlap);
                    (u32::from(lut.class(c) != class), stats)
                })
                .reduce(|| (0, LutStats::default()), |a, b| (a.0 + b.0, a.1.merge(b.1)));
            if mismatches > 0 {
                let first: Vec<_> = (0..COLORS as u32)
                    .filter(|&c| lut.class(c) != reference::evaluate(&rules, Rgb::from_u32(c)).0)
                    .take(5)
                    .map(|c| {
                        (
                            Rgb::from_u32(c),
                            lut.class(c),
                            reference::evaluate(&rules, Rgb::from_u32(c)).0,
                        )
                    })
                    .collect();
                panic!("{name}: {mismatches} colors differ from the reference, first: {first:?}");
            }
            assert_eq!(lut.stats(), stats, "{name}");
            all.push(stats);
        }
        // The cases must exercise every class, or the comparison above proves little.
        assert!(
            all[0].tight > 0 && all[0].wide > 0 && all[0].excluded > 0,
            "{:?}",
            all[0]
        );
        assert!(all[1].overlap > 0, "{:?}", all[1]);
        assert!(all.iter().any(|s| s.tight > 1_000_000));
        assert_eq!(all[7], LutStats::default());
    }

    #[test]
    fn packing_at_word_boundaries() {
        let rules = ColorRules {
            model: ColorModel::Rgb,
            include: vec![
                entry("#00001F", |e| e.tol_rgb = Some(0)),
                entry("#000020", |e| e.tol_rgb = Some(0)),
            ],
            exclude: vec![entry("#FFFFFF", |e| e.tol_rgb = Some(0))],
            ..ColorRules::default()
        };
        let lut = Lut::build(&CompiledRules::new(&rules));
        assert_eq!(lut.class(0x00001E), Class::Absent);
        assert_eq!(lut.class(0x00001F), Class::Tight); // last entry of word 0
        assert_eq!(lut.class(0x000020), Class::Tight); // first entry of word 1
        assert_eq!(lut.class(0x000021), Class::Absent);
        assert_eq!(lut.class(0xFFFFFF), Class::Excluded); // last entry of the table
        assert_eq!(lut.class(0xFF00_001F), Class::Tight); // bits above 24 are ignored
        assert_eq!(
            lut.stats(),
            LutStats {
                tight: 2,
                wide: 0,
                excluded: 1,
                overlap: 0
            }
        );
    }
}
