// SPDX-License-Identifier: MIT
//! The HSV color model (PLAN §5.4). The RGB-box model needs no conversion; its test lives in
//! `rule.rs`.

use super::Rgb;

/// Below this chroma (`(max − min) / 255`), an HSV hue is meaningless (ADR 0005).
pub const HSV_ACHROMATIC_CHROMA: f64 = 0.02;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Hsv {
    /// Hue in degrees, `[0, 360)`; 0 for grays.
    pub h: f64,
    /// Saturation, 0–1.
    pub s: f64,
    /// Value (brightness), 0–1.
    pub v: f64,
    /// Chroma, `(max − min) / 255`: how colorful the pixel is, whatever its brightness. Unlike
    /// saturation it stays near 0 for dark noise such as `#030101`.
    pub chroma: f64,
}

impl Hsv {
    pub fn from_rgb(rgb: Rgb) -> Self {
        let (r, g, b) = (i32::from(rgb.r()), i32::from(rgb.g()), i32::from(rgb.b()));
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        let h = if delta == 0 {
            0.0
        } else {
            let d = f64::from(delta);
            let sector = if max == r {
                f64::from(g - b) / d
            } else if max == g {
                f64::from(b - r) / d + 2.0
            } else {
                f64::from(r - g) / d + 4.0
            };
            let h = sector * 60.0;
            if h < 0.0 { h + 360.0 } else { h }
        };
        let s = if max == 0 {
            0.0
        } else {
            f64::from(delta) / f64::from(max)
        };
        Self {
            h,
            s,
            v: f64::from(max) / 255.0,
            chroma: f64::from(delta) / 255.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primaries_and_secondaries() {
        for (rgb, h) in [
            (Rgb::new(255, 0, 0), 0.0),
            (Rgb::new(255, 255, 0), 60.0),
            (Rgb::new(0, 255, 0), 120.0),
            (Rgb::new(0, 255, 255), 180.0),
            (Rgb::new(0, 0, 255), 240.0),
            (Rgb::new(255, 0, 255), 300.0),
        ] {
            let hsv = Hsv::from_rgb(rgb);
            assert_eq!((hsv.h, hsv.s, hsv.v, hsv.chroma), (h, 1.0, 1.0, 1.0), "{rgb}");
        }
    }

    #[test]
    fn hue_just_below_red_wraps_to_the_top() {
        let hsv = Hsv::from_rgb(Rgb::new(255, 0, 10));
        assert!(hsv.h > 357.0 && hsv.h < 360.0, "{hsv:?}");
    }

    #[test]
    fn grays_and_black() {
        let gray = Hsv::from_rgb(Rgb::new(128, 128, 128));
        assert_eq!((gray.h, gray.s, gray.chroma), (0.0, 0.0, 0.0));
        assert!((gray.v - 128.0 / 255.0).abs() < 1e-12);
        let black = Hsv::from_rgb(Rgb::new(0, 0, 0));
        assert_eq!((black.h, black.s, black.v, black.chroma), (0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn dark_noise_is_saturated_but_not_colorful() {
        let hsv = Hsv::from_rgb(Rgb::new(3, 1, 1));
        assert!(hsv.s > 0.6);
        assert!(hsv.chroma < HSV_ACHROMATIC_CHROMA);
    }

    #[test]
    fn a_green_from_the_plan() {
        let hsv = Hsv::from_rgb(Rgb::new(0x3B, 0xD1, 0x6F));
        // max = g = 209, min = r = 59: h = 60 * ((111 - 59) / 150 + 2) = 140.8
        assert!((hsv.h - 140.8).abs() < 1e-9, "{hsv:?}");
        assert!((hsv.s - 150.0 / 209.0).abs() < 1e-12);
        assert!((hsv.v - 209.0 / 255.0).abs() < 1e-12);
    }
}
