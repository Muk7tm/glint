// SPDX-License-Identifier: MIT
//! OKLab/OKLCh reference math (PLAN §5.4, Björn Ottosson's definition).
//!
//! Everything that converts colors (the lookup-table builder, the tests, the color picker)
//! goes through these functions, so they all agree to the bit.

use std::sync::LazyLock;

use super::Rgb;

/// Below this chroma, hue is meaningless (grays, near-white, near-black; PLAN §5.4).
pub const ACHROMATIC_CHROMA: f64 = 0.02;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Oklab {
    pub l: f64,
    pub a: f64,
    pub b: f64,
}

/// The cylindrical form of OKLab: lightness 0–1, chroma (colorfulness), hue in degrees.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Oklch {
    pub l: f64,
    pub c: f64,
    pub h: f64,
}

/// The sRGB transfer function for one 8-bit channel.
pub fn linearize(channel: u8) -> f64 {
    let v = f64::from(channel) / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

static LINEAR: LazyLock<[f64; 256]> = LazyLock::new(|| std::array::from_fn(|i| linearize(i as u8)));

/// [`linearize`] for all 256 channel values.
pub fn linear_table() -> &'static [f64; 256] {
    &LINEAR
}

impl Oklab {
    pub fn from_rgb(rgb: Rgb) -> Self {
        let lin = linear_table();
        Self::from_linear(
            lin[usize::from(rgb.r())],
            lin[usize::from(rgb.g())],
            lin[usize::from(rgb.b())],
        )
    }

    /// From linear-light sRGB.
    #[inline]
    pub fn from_linear(r: f64, g: f64, b: f64) -> Self {
        let l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
        let m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
        let s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;
        let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
        Self {
            l: 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
            a: 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
            b: 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
        }
    }

    #[inline]
    pub fn chroma(self) -> f64 {
        (self.a * self.a + self.b * self.b).sqrt()
    }

    #[inline]
    pub fn hue(self) -> f64 {
        hue_degrees(self.a, self.b)
    }

    pub fn to_lch(self) -> Oklch {
        Oklch {
            l: self.l,
            c: self.chroma(),
            h: self.hue(),
        }
    }
}

impl Oklch {
    pub fn from_rgb(rgb: Rgb) -> Self {
        Oklab::from_rgb(rgb).to_lch()
    }

    /// The nearest sRGB color (channels clamped), for drawing a range: the UI's shade and hue
    /// strips (§12.4).
    pub fn to_rgb(self) -> Rgb {
        let (a, b) = (self.c * self.h.to_radians().cos(), self.c * self.h.to_radians().sin());
        let l_ = self.l + 0.3963377774 * a + 0.2158037573 * b;
        let m_ = self.l - 0.1055613458 * a - 0.0638541728 * b;
        let s_ = self.l - 0.0894841775 * a - 1.2914855480 * b;
        let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
        let r = 4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s;
        let g = -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s;
        let bl = -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s;
        let encode = |v: f64| {
            let v = v.clamp(0.0, 1.0);
            let e = if v <= 0.0031308 {
                12.92 * v
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            };
            (e * 255.0).round().clamp(0.0, 255.0) as u8
        };
        Rgb::new(encode(r), encode(g), encode(bl))
    }
}

/// The angle of `(a, b)` in degrees, in `[0, 360]`.
#[inline]
pub fn hue_degrees(a: f64, b: f64) -> f64 {
    let h = b.atan2(a).to_degrees();
    if h < 0.0 { h + 360.0 } else { h }
}

/// The shorter way around the hue circle, in degrees: 355° and 5° are 10° apart.
#[inline]
pub fn hue_distance(h1: f64, h2: f64) -> f64 {
    let d = (h1 - h2).abs() % 360.0;
    d.min(360.0 - d)
}

#[cfg(test)]
mod tests {
    use palette::{IntoColor, Srgb};

    use super::*;

    fn close(x: f64, y: f64, eps: f64) -> bool {
        (x - y).abs() <= eps
    }

    #[test]
    fn back_to_rgb() {
        for hex in [
            0x86_04_A0u32,
            0x3B_D1_6F,
            0xFF_FF_FF,
            0x00_00_00,
            0xE0_30_78,
            0x12_34_56,
            0xCB_49_DE,
        ] {
            let rgb = Rgb::from_u32(hex);
            let back = Oklch::from_rgb(rgb).to_rgb();
            for (x, y) in [(rgb.r(), back.r()), (rgb.g(), back.g()), (rgb.b(), back.b())] {
                assert!(x.abs_diff(y) <= 1, "{hex:06X} came back as {back:?}");
            }
        }
        // Out of gamut: clamped, not wrapped.
        let vivid = Oklch {
            l: 0.9,
            c: 0.5,
            h: 140.0,
        }
        .to_rgb();
        assert!(vivid.g() > 200, "{vivid:?}");
    }

    #[test]
    fn plan_test_vectors() {
        // PLAN §5.4, given to four decimals.
        for (rgb, l, a, b) in [
            (Rgb::new(255, 255, 255), 1.0, 0.0, 0.0),
            (Rgb::new(255, 0, 0), 0.6280, 0.2249, 0.1258),
            (Rgb::new(0, 0, 255), 0.4520, -0.0325, -0.3115),
        ] {
            let lab = Oklab::from_rgb(rgb);
            assert!(
                close(lab.l, l, 5e-5) && close(lab.a, a, 5e-5) && close(lab.b, b, 5e-5),
                "{rgb}: {lab:?}"
            );
        }
    }

    #[test]
    fn agrees_with_the_palette_crate() {
        // Every 5th value per channel, including both ends: 52³ colors.
        for r in (0..=255u8).step_by(5) {
            for g in (0..=255u8).step_by(5) {
                for b in (0..=255u8).step_by(5) {
                    let ours = Oklab::from_rgb(Rgb::new(r, g, b));
                    let theirs: palette::Oklab<f64> = Srgb::new(r, g, b).into_format::<f64>().into_color();
                    assert!(
                        close(ours.l, theirs.l, 1e-9) && close(ours.a, theirs.a, 1e-9) && close(ours.b, theirs.b, 1e-9),
                        "{}: ours {ours:?}, palette {theirs:?}",
                        Rgb::new(r, g, b)
                    );
                }
            }
        }
    }

    #[test]
    fn linear_table_matches_the_formula() {
        for c in 0..=255u8 {
            assert_eq!(linear_table()[usize::from(c)].to_bits(), linearize(c).to_bits());
        }
        assert_eq!(linearize(0), 0.0);
        assert_eq!(linearize(255), 1.0);
    }

    #[test]
    fn grays_have_no_chroma_but_a_noisy_hue() {
        // The reason for the achromatic guard: a gray's hue is rounding noise, not a color.
        for v in [0u8, 1, 128, 254, 255] {
            let lch = Oklch::from_rgb(Rgb::new(v, v, v));
            assert!(lch.c < 1e-6, "gray {v}: {lch:?}");
        }
        let white = Oklch::from_rgb(Rgb::new(255, 255, 255));
        assert!(white.h > 1.0, "white's hue is {}, not 0", white.h);
    }

    #[test]
    fn hue_distance_wraps() {
        assert!(close(hue_distance(355.0, 5.0), 10.0, 1e-12));
        assert!(close(hue_distance(5.0, 355.0), 10.0, 1e-12));
        assert!(close(hue_distance(0.0, 180.0), 180.0, 1e-12));
        assert!(close(hue_distance(360.0, 0.0), 0.0, 1e-12));
        assert!(close(hue_distance(10.0, 20.0), 10.0, 1e-12));
    }

    #[test]
    fn hue_is_in_range() {
        for (a, b) in [(1.0, 0.0), (-1.0, 0.0), (0.0, -1.0), (1.0, -1e-300), (-1.0, -1e-300)] {
            let h = hue_degrees(a, b);
            assert!((0.0..=360.0).contains(&h), "({a}, {b}) -> {h}");
        }
    }
}
