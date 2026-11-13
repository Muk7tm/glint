// SPDX-License-Identifier: GPL-3.0-or-later
//! Include and exclude colors with tight and wide ranges (PLAN §5.4, ADR 0005), and their
//! compiled form, which the lookup-table builder runs on every 24-bit color.
//!
//! A pixel is **excluded** if it lies in any exclude range: exclusion always wins. Otherwise it
//! is **tight** if it lies in any include color's tight range (the tolerances), **wide** if it
//! lies in any include color's wide range (the tolerances × `exit_margin`), and **absent**
//! otherwise. Because `exit_margin ≥ 1`, every tight range lies inside its wide range.

use serde::{Deserialize, Serialize};

use super::hsv::{HSV_ACHROMATIC_CHROMA, Hsv};
use super::oklab::{ACHROMATIC_CHROMA, Oklab, Oklch, hue_degrees, hue_distance};
use super::{Class, Rgb};

pub const DEFAULT_EXIT_MARGIN: f64 = 1.5;

/// Missing tolerances of an include color take this preset's values.
pub const INCLUDE_PRESET: ShadePreset = ShadePreset::Normal;

/// Missing tolerances of an exclude color take this preset's values: an exclusion is a small
/// range around one near-miss color (ADR 0005).
pub const EXCLUDE_PRESET: ShadePreset = ShadePreset::Tight;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorModel {
    /// Lightness ± ΔL, chroma ± ΔC and hue ± Δh in OKLCh.
    #[default]
    Oklch,
    /// Hue ± ΔH, saturation ± ΔS and value ± ΔV.
    Hsv,
    /// Each channel ± t.
    Rgb,
}

/// The *Shades* control: starting tolerances for a color.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShadePreset {
    Tight,
    #[default]
    Normal,
    Loose,
}

/// One color's tolerances, resolved for a model.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tolerances {
    /// Lightness (0–1), chroma, hue (degrees).
    Oklch { l: f64, c: f64, h: f64 },
    /// Hue (degrees), saturation (0–1), value (0–1).
    Hsv { h: f64, s: f64, v: f64 },
    /// Per channel (0–255).
    Rgb { t: f64 },
}

impl ShadePreset {
    /// The OKLCh values are the plan's (§5.4); the HSV and RGB values are ADR 0005's.
    pub fn tolerances(self, model: ColorModel) -> Tolerances {
        let i = match self {
            Self::Tight => 0,
            Self::Normal => 1,
            Self::Loose => 2,
        };
        match model {
            ColorModel::Oklch => {
                let (l, c, h) = [(0.05, 0.03, 4.0), (0.10, 0.05, 7.0), (0.16, 0.08, 12.0)][i];
                Tolerances::Oklch { l, c, h }
            }
            ColorModel::Hsv => {
                let (h, s, v) = [(4.0, 0.08, 0.08), (7.0, 0.15, 0.15), (12.0, 0.25, 0.25)][i];
                Tolerances::Hsv { h, s, v }
            }
            ColorModel::Rgb => Tolerances::Rgb {
                t: [6.0, 12.0, 20.0][i],
            },
        }
    }

    /// A color entry with this preset's tolerances for `model` written out.
    pub fn entry(self, model: ColorModel, hex: Rgb) -> ColorEntry {
        let mut entry = ColorEntry::new(hex);
        match self.tolerances(model) {
            Tolerances::Oklch { l, c, h } => {
                entry.tol_l = Some(l);
                entry.tol_c = Some(c);
                entry.tol_h = Some(h);
            }
            Tolerances::Hsv { h, s, v } => {
                entry.tol_h = Some(h);
                entry.tol_s = Some(s);
                entry.tol_v = Some(v);
            }
            Tolerances::Rgb { t } => entry.tol_rgb = Some(t as u8),
        }
        entry
    }
}

/// One include or exclude color. Only the tolerances of the profile's model are used; the
/// others are kept, so switching models back and forth loses nothing (ADR 0005).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColorEntry {
    pub hex: Rgb,
    /// OKLCh lightness ± (0–1; the UI shows it × 100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tol_l: Option<f64>,
    /// OKLCh chroma ± (the UI shows it × 100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tol_c: Option<f64>,
    /// Hue ± in degrees (OKLCh and HSV).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tol_h: Option<f64>,
    /// HSV saturation ± (0–1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tol_s: Option<f64>,
    /// HSV value ± (0–1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tol_v: Option<f64>,
    /// RGB ± per channel (0–255; 0 matches only the exact color).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tol_rgb: Option<u8>,
}

impl ColorEntry {
    /// An entry whose tolerances all come from the presets.
    pub fn new(hex: Rgb) -> Self {
        Self {
            hex,
            tol_l: None,
            tol_c: None,
            tol_h: None,
            tol_s: None,
            tol_v: None,
            tol_rgb: None,
        }
    }

    /// This entry's tolerances under `model`; a missing field takes `preset`'s value.
    pub fn tolerances(&self, model: ColorModel, preset: ShadePreset) -> Tolerances {
        match preset.tolerances(model) {
            Tolerances::Oklch { l, c, h } => Tolerances::Oklch {
                l: self.tol_l.unwrap_or(l),
                c: self.tol_c.unwrap_or(c),
                h: self.tol_h.unwrap_or(h),
            },
            Tolerances::Hsv { h, s, v } => Tolerances::Hsv {
                h: self.tol_h.unwrap_or(h),
                s: self.tol_s.unwrap_or(s),
                v: self.tol_v.unwrap_or(v),
            },
            Tolerances::Rgb { t } => Tolerances::Rgb {
                t: self.tol_rgb.map_or(t, f64::from),
            },
        }
    }
}

fn default_exit_margin() -> f64 {
    DEFAULT_EXIT_MARGIN
}

/// The `[color]` section of a profile.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColorRules {
    #[serde(default)]
    pub model: ColorModel,
    /// Wide range = tight range × `exit_margin` (1.0–3.0).
    #[serde(default = "default_exit_margin")]
    pub exit_margin: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<ColorEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<ColorEntry>,
}

impl Default for ColorRules {
    fn default() -> Self {
        Self {
            model: ColorModel::default(),
            exit_margin: DEFAULT_EXIT_MARGIN,
            include: Vec::new(),
            exclude: Vec::new(),
        }
    }
}

/// True when hue is ignored for `hex` under `model` because the color is (nearly) gray. The UI
/// warns that grays are hard to tell apart from backgrounds (§5.4).
pub fn is_achromatic(model: ColorModel, hex: Rgb) -> bool {
    match model {
        ColorModel::Oklch => Oklch::from_rgb(hex).c < ACHROMATIC_CHROMA,
        ColorModel::Hsv => Hsv::from_rgb(hex).chroma < HSV_ACHROMATIC_CHROMA,
        ColorModel::Rgb => false,
    }
}

/// A rule set prepared for testing single colors.
///
/// The lookup-table builder runs this on all 16,777,216 colors; the hot loop only ever reads
/// the finished table.
#[derive(Clone, Debug)]
pub struct CompiledRules(Kind);

#[derive(Clone, Debug)]
enum Kind {
    Oklch {
        include: Vec<Ranges<OklchBox>>,
        exclude: Vec<OklchBox>,
    },
    Hsv {
        include: Vec<Ranges<HsvBox>>,
        exclude: Vec<HsvBox>,
    },
    Rgb {
        include: Vec<Ranges<RgbBox>>,
        exclude: Vec<RgbBox>,
    },
}

/// An include color's two ranges; `tight` lies inside `wide`.
#[derive(Clone, Copy, Debug)]
struct Ranges<B> {
    tight: B,
    wide: B,
}

trait Range {
    type Pixel;
    fn contains(&self, p: &Self::Pixel) -> bool;
}

impl CompiledRules {
    pub fn new(rules: &ColorRules) -> Self {
        Self(match rules.model {
            ColorModel::Oklch => Kind::Oklch {
                include: compile_include(rules, OklchBox::new),
                exclude: compile_exclude(rules, OklchBox::new),
            },
            ColorModel::Hsv => Kind::Hsv {
                include: compile_include(rules, HsvBox::new),
                exclude: compile_exclude(rules, HsvBox::new),
            },
            ColorModel::Rgb => Kind::Rgb {
                include: compile_include(rules, RgbBox::new),
                exclude: compile_exclude(rules, RgbBox::new),
            },
        })
    }

    /// The class of one color.
    pub fn classify(&self, rgb: Rgb) -> Class {
        self.evaluate(rgb).0
    }

    /// The class of one color, and for an excluded color whether an include range would have
    /// matched it (the UI warns when an exclusion cuts into an include range, §5.4).
    #[inline]
    pub fn evaluate(&self, rgb: Rgb) -> (Class, bool) {
        match &self.0 {
            Kind::Oklch { include, exclude } => {
                let lab = Oklab::from_rgb(rgb);
                evaluate(
                    include,
                    exclude,
                    &OklchPixel {
                        l: lab.l,
                        a: lab.a,
                        b: lab.b,
                        c: lab.chroma(),
                    },
                )
            }
            Kind::Hsv { include, exclude } => evaluate(include, exclude, &Hsv::from_rgb(rgb)),
            Kind::Rgb { include, exclude } => evaluate(include, exclude, &rgb),
        }
    }
}

fn compile_include<B>(rules: &ColorRules, build: fn(Rgb, Tolerances, f64) -> B) -> Vec<Ranges<B>> {
    // Validation keeps exit_margin in 1.0–3.0; never let the wide range be the smaller one.
    let margin = rules.exit_margin.max(1.0);
    let range = |e: &ColorEntry| {
        let tol = e.tolerances(rules.model, INCLUDE_PRESET);
        Ranges {
            tight: build(e.hex, tol, 1.0),
            wide: build(e.hex, tol, margin),
        }
    };
    rules.include.iter().map(range).collect()
}

fn compile_exclude<B>(rules: &ColorRules, build: fn(Rgb, Tolerances, f64) -> B) -> Vec<B> {
    rules
        .exclude
        .iter()
        .map(|e| build(e.hex, e.tolerances(rules.model, EXCLUDE_PRESET), 1.0))
        .collect()
}

#[inline]
fn evaluate<B: Range>(include: &[Ranges<B>], exclude: &[B], p: &B::Pixel) -> (Class, bool) {
    let included = || {
        let mut class = Class::Absent;
        for r in include {
            if r.wide.contains(p) {
                if r.tight.contains(p) {
                    return Class::Tight;
                }
                class = Class::Wide;
            }
        }
        class
    };
    if exclude.iter().any(|x| x.contains(p)) {
        (Class::Excluded, included() != Class::Absent)
    } else {
        (included(), false)
    }
}

struct OklchPixel {
    l: f64,
    a: f64,
    b: f64,
    c: f64,
}

#[derive(Clone, Copy, Debug)]
struct OklchBox {
    l: f64,
    c: f64,
    h: f64,
    /// Gray target: hue is ignored, only L and C count (§5.4).
    achromatic: bool,
    dl: f64,
    dc: f64,
    dh: f64,
}

impl OklchBox {
    fn new(hex: Rgb, tol: Tolerances, scale: f64) -> Self {
        let t = Oklch::from_rgb(hex);
        let (l, c, h) = match tol {
            Tolerances::Oklch { l, c, h } => (l, c, h),
            _ => (f64::NAN, f64::NAN, f64::NAN), // unreachable: tolerances come from the same model
        };
        Self {
            l: t.l,
            c: t.c,
            h: t.h,
            achromatic: t.c < ACHROMATIC_CHROMA,
            dl: l * scale,
            dc: c * scale,
            dh: h * scale,
        }
    }
}

impl Range for OklchBox {
    type Pixel = OklchPixel;

    #[inline]
    fn contains(&self, p: &OklchPixel) -> bool {
        (p.l - self.l).abs() <= self.dl
            && (p.c - self.c).abs() <= self.dc
            // A gray pixel's hue is noise: it never passes a colored target's hue test (ADR 0005).
            && (self.achromatic || (p.c >= ACHROMATIC_CHROMA && hue_distance(hue_degrees(p.a, p.b), self.h) <= self.dh))
    }
}

#[derive(Clone, Copy, Debug)]
struct HsvBox {
    h: f64,
    s: f64,
    v: f64,
    achromatic: bool,
    dh: f64,
    ds: f64,
    dv: f64,
}

impl HsvBox {
    fn new(hex: Rgb, tol: Tolerances, scale: f64) -> Self {
        let t = Hsv::from_rgb(hex);
        let (h, s, v) = match tol {
            Tolerances::Hsv { h, s, v } => (h, s, v),
            _ => (f64::NAN, f64::NAN, f64::NAN), // unreachable: tolerances come from the same model
        };
        Self {
            h: t.h,
            s: t.s,
            v: t.v,
            achromatic: t.chroma < HSV_ACHROMATIC_CHROMA,
            dh: h * scale,
            ds: s * scale,
            dv: v * scale,
        }
    }
}

impl Range for HsvBox {
    type Pixel = Hsv;

    #[inline]
    fn contains(&self, p: &Hsv) -> bool {
        (p.v - self.v).abs() <= self.dv
            && (p.s - self.s).abs() <= self.ds
            && (self.achromatic || (p.chroma >= HSV_ACHROMATIC_CHROMA && hue_distance(p.h, self.h) <= self.dh))
    }
}

#[derive(Clone, Copy, Debug)]
struct RgbBox {
    r: i32,
    g: i32,
    b: i32,
    t: f64,
}

impl RgbBox {
    fn new(hex: Rgb, tol: Tolerances, scale: f64) -> Self {
        let t = match tol {
            Tolerances::Rgb { t } => t,
            _ => f64::NAN, // unreachable: tolerances come from the same model
        };
        Self {
            r: i32::from(hex.r()),
            g: i32::from(hex.g()),
            b: i32::from(hex.b()),
            t: t * scale,
        }
    }
}

impl Range for RgbBox {
    type Pixel = Rgb;

    #[inline]
    fn contains(&self, p: &Rgb) -> bool {
        f64::from((i32::from(p.r()) - self.r).abs()) <= self.t
            && f64::from((i32::from(p.g()) - self.g).abs()) <= self.t
            && f64::from((i32::from(p.b()) - self.b).abs()) <= self.t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(s: &str) -> Rgb {
        s.parse().unwrap()
    }

    fn rules(model: ColorModel, include: &[ColorEntry], exclude: &[ColorEntry]) -> CompiledRules {
        CompiledRules::new(&ColorRules {
            model,
            exit_margin: DEFAULT_EXIT_MARGIN,
            include: include.to_vec(),
            exclude: exclude.to_vec(),
        })
    }

    fn normal(model: ColorModel, hex: &str) -> ColorEntry {
        ShadePreset::Normal.entry(model, rgb(hex))
    }

    #[test]
    fn the_target_itself_is_tight_in_every_model_and_preset() {
        for model in [ColorModel::Oklch, ColorModel::Hsv, ColorModel::Rgb] {
            for preset in [ShadePreset::Tight, ShadePreset::Normal, ShadePreset::Loose] {
                for hex in ["#3BD16F", "#808080", "#000000", "#FFFFFF", "#E03078"] {
                    let r = rules(model, &[preset.entry(model, rgb(hex))], &[]);
                    assert_eq!(r.classify(rgb(hex)), Class::Tight, "{model:?} {preset:?} {hex}");
                }
            }
        }
    }

    #[test]
    fn oklch_shades_of_one_green() {
        // Darker shades of #3BD16F along the same hue (computed offsets in the comments).
        let r = rules(ColorModel::Oklch, &[normal(ColorModel::Oklch, "#3BD16F")], &[]);
        assert_eq!(r.classify(rgb("#33B861")), Class::Tight); // ΔL −0.069
        assert_eq!(r.classify(rgb("#2DA657")), Class::Wide); // ΔL −0.120: between ×1 and ×1.5
        assert_eq!(r.classify(rgb("#26924C")), Class::Absent); // ΔL −0.178
    }

    #[test]
    fn a_similar_hue_never_matches() {
        // R5: the yellow-green #6BD13B is 12.8° of hue away from #3BD16F, outside even the
        // wide range (7° × 1.5), although its lightness and chroma are close.
        let r = rules(ColorModel::Oklch, &[normal(ColorModel::Oklch, "#3BD16F")], &[]);
        assert_eq!(r.classify(rgb("#6BD13B")), Class::Absent);
    }

    #[test]
    fn exclusion_always_wins() {
        let green = normal(ColorModel::Oklch, "#3BD16F");
        let r = rules(
            ColorModel::Oklch,
            std::slice::from_ref(&green),
            &[ColorEntry::new(rgb("#3BD16F"))],
        );
        assert_eq!(r.evaluate(rgb("#3BD16F")), (Class::Excluded, true));
        // A color the exclusion covers but no include range does reports no overlap.
        let r = rules(ColorModel::Oklch, &[green], &[ColorEntry::new(rgb("#E03030"))]);
        assert_eq!(r.evaluate(rgb("#E03030")), (Class::Excluded, false));
        assert_eq!(r.classify(rgb("#3BD16F")), Class::Tight);
    }

    #[test]
    fn several_include_colors() {
        let r = rules(
            ColorModel::Oklch,
            &[
                normal(ColorModel::Oklch, "#3BD16F"),
                normal(ColorModel::Oklch, "#E03030"),
            ],
            &[],
        );
        assert_eq!(r.classify(rgb("#3BD16F")), Class::Tight);
        assert_eq!(r.classify(rgb("#E03030")), Class::Tight);
        assert_eq!(r.classify(rgb("#0000FF")), Class::Absent);
        // Tight in one range beats wide in another.
        let r = rules(
            ColorModel::Oklch,
            &[
                normal(ColorModel::Oklch, "#2DA657"),
                normal(ColorModel::Oklch, "#3BD16F"),
            ],
            &[],
        );
        assert_eq!(r.classify(rgb("#3BD16F")), Class::Tight);
    }

    #[test]
    fn a_gray_target_ignores_hue() {
        let r = rules(ColorModel::Oklch, &[normal(ColorModel::Oklch, "#808080")], &[]);
        assert!(is_achromatic(ColorModel::Oklch, rgb("#808080")));
        assert_eq!(r.classify(rgb("#858585")), Class::Tight); // ΔL 0.017
        assert_eq!(r.classify(rgb("#808A80")), Class::Tight); // a faint green tint, C 0.019
        assert_eq!(r.classify(rgb("#3BD16F")), Class::Absent); // far too colorful
    }

    #[test]
    fn a_gray_pixel_never_passes_a_colored_targets_hue_test() {
        // #7A8A7E is a muted green (C 0.026, h 153.1°). #7F8881 has nearly the same lightness,
        // chroma and hue, but its chroma (0.015) is below the achromatic threshold, so its hue
        // is noise and it doesn't match (ADR 0005). #7A8A7F (C 0.025, h 155.6°) does.
        let r = rules(ColorModel::Oklch, &[normal(ColorModel::Oklch, "#7A8A7E")], &[]);
        assert!(!is_achromatic(ColorModel::Oklch, rgb("#7A8A7E")));
        assert_eq!(r.classify(rgb("#7F8881")), Class::Absent);
        assert_eq!(r.classify(rgb("#7A8A7F")), Class::Tight);
        // Pure grays of the same lightness: their hue is rounding noise.
        assert_eq!(r.classify(rgb("#858585")), Class::Absent);
    }

    #[test]
    fn hue_wraps_around_zero() {
        // #E03078 has h 1.99°, #E03080 has h 358.93°: 3.06° apart.
        let mut e = normal(ColorModel::Oklch, "#E03078");
        e.tol_h = Some(4.0);
        let r = rules(ColorModel::Oklch, &[e.clone()], &[]);
        assert_eq!(r.classify(rgb("#E03080")), Class::Tight);
        e.tol_h = Some(2.5); // 3.06° is outside 2.5° but inside 2.5° × 1.5
        let r = rules(ColorModel::Oklch, &[e], &[]);
        assert_eq!(r.classify(rgb("#E03080")), Class::Wide);
    }

    #[test]
    fn hsv_ranges() {
        // #3BD16F: h 140.8°, s 0.718, v 0.820.
        let r = rules(ColorModel::Hsv, &[normal(ColorModel::Hsv, "#3BD16F")], &[]);
        assert_eq!(r.classify(rgb("#38C76A")), Class::Tight); // v 0.780: Δv −0.04
        assert_eq!(r.classify(rgb("#2FA85A")), Class::Wide); // v 0.659: Δv −0.161
        assert_eq!(r.classify(rgb("#6BD13B")), Class::Absent); // h 100.8°
        assert_eq!(r.classify(rgb("#D1D1D1")), Class::Absent);
        // A gray target ignores hue, and a gray pixel fails a colored target's hue test.
        let r = rules(ColorModel::Hsv, &[normal(ColorModel::Hsv, "#808080")], &[]);
        assert_eq!(r.classify(rgb("#828084")), Class::Tight);
    }

    #[test]
    fn rgb_box_ranges() {
        let mut e = ColorEntry::new(rgb("#808080"));
        e.tol_rgb = Some(12);
        let r = rules(ColorModel::Rgb, &[e.clone()], &[]);
        assert_eq!(r.classify(rgb("#8C8080")), Class::Tight); // +12
        assert_eq!(r.classify(rgb("#807392")), Class::Wide); // −13 and +18: within 12 × 1.5
        assert_eq!(r.classify(rgb("#808093")), Class::Absent); // +19
        e.tol_rgb = Some(0); // exact-color matching
        let r = rules(ColorModel::Rgb, &[e], &[]);
        assert_eq!(r.classify(rgb("#808080")), Class::Tight);
        assert_eq!(r.classify(rgb("#808081")), Class::Absent);
    }

    #[test]
    fn exit_margin_of_one_means_no_wide_band() {
        let r = CompiledRules::new(&ColorRules {
            exit_margin: 1.0,
            include: vec![normal(ColorModel::Oklch, "#3BD16F")],
            ..ColorRules::default()
        });
        assert_eq!(r.classify(rgb("#33B861")), Class::Tight);
        assert_eq!(r.classify(rgb("#2DA657")), Class::Absent);
    }

    #[test]
    fn no_include_colors_match_nothing() {
        let r = CompiledRules::new(&ColorRules {
            exclude: vec![ColorEntry::new(rgb("#000000"))],
            ..ColorRules::default()
        });
        assert_eq!(r.classify(rgb("#3BD16F")), Class::Absent);
        assert_eq!(r.evaluate(rgb("#000000")), (Class::Excluded, false));
    }

    #[test]
    fn missing_tolerances_take_the_presets() {
        let e = ColorEntry::new(rgb("#3BD16F"));
        assert_eq!(
            e.tolerances(ColorModel::Oklch, INCLUDE_PRESET),
            Tolerances::Oklch {
                l: 0.10,
                c: 0.05,
                h: 7.0
            }
        );
        assert_eq!(
            e.tolerances(ColorModel::Oklch, EXCLUDE_PRESET),
            Tolerances::Oklch {
                l: 0.05,
                c: 0.03,
                h: 4.0
            }
        );
        let mut e = e;
        e.tol_h = Some(9.0);
        assert_eq!(
            e.tolerances(ColorModel::Hsv, INCLUDE_PRESET),
            Tolerances::Hsv {
                h: 9.0,
                s: 0.15,
                v: 0.15
            }
        );
        assert_eq!(
            e.tolerances(ColorModel::Rgb, INCLUDE_PRESET),
            Tolerances::Rgb { t: 12.0 }
        );
    }

    #[test]
    fn color_section_toml_round_trip() {
        // Appendix B's [color] section.
        let text = r##"
            model = "oklch"
            exit_margin = 1.5

            [[include]]
            hex = "#3BD16F"
            tol_l = 0.10
            tol_c = 0.05
            tol_h = 7.0

            [[exclude]]
            hex = "#6bd13b"
            tol_l = 0.05
            tol_c = 0.03
            tol_h = 4.0
        "##;
        let parsed: ColorRules = toml::from_str(text).unwrap();
        assert_eq!(parsed.include, vec![normal(ColorModel::Oklch, "#3BD16F")]);
        assert_eq!(
            parsed.exclude,
            vec![ShadePreset::Tight.entry(ColorModel::Oklch, rgb("#6BD13B"))]
        );
        let again: ColorRules = toml::from_str(&toml::to_string(&parsed).unwrap()).unwrap();
        assert_eq!(again, parsed);
        // Defaults, and typos are errors rather than silently ignored.
        let minimal: ColorRules = toml::from_str("[[include]]\nhex = \"#3BD16F\"").unwrap();
        assert_eq!((minimal.model, minimal.exit_margin), (ColorModel::Oklch, 1.5));
        assert!(toml::from_str::<ColorRules>("[[include]]\nhex = \"#3BD16F\"\ntol_hue = 7.0").is_err());
        assert!(toml::from_str::<ColorRules>("model = \"lab\"").is_err());
    }
}
