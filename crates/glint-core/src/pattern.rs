// SPDX-License-Identifier: MIT
//! Sample patterns and pixel agreement (PLAN §5.3, ADR 0006).
//!
//! A pattern is a set of pixel offsets around the anchor pixel. The engine captures the
//! pattern's bounding rectangle with a single `GetImage`, so a bigger pattern costs no extra
//! round trip, and [`SamplePattern::verdict`] turns those pixels into Enter, Present or Absent.

use serde::{Deserialize, Serialize};

use crate::color::Lut;

/// The most pixels a pattern can have.
pub const MAX_POINTS: usize = 64;

/// How far a pattern pixel may sit from the anchor on either axis.
pub const MAX_REACH: u16 = 31;

/// The plus pattern's arm length when the profile doesn't set one.
pub const DEFAULT_PLUS_DISTANCE: u16 = 2;

/// The result of one check (§5.3). `Enter` implies `Present`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Neither condition below holds.
    Absent,
    /// At least `k_stay` pixels are in the wide range: the color is still there.
    Present,
    /// At least `k_enter` pixels are in the tight range: enough to start an appearance.
    Enter,
}

impl Verdict {
    pub fn present(self) -> bool {
        self != Self::Absent
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PatternKind {
    /// The anchor pixel alone.
    Pixel,
    /// The 2×2 block around the true center of an even-sized area.
    Center2x2,
    #[default]
    Square3,
    Square5,
    /// The anchor plus four pixels at `plus_distance` left, right, above and below.
    Plus,
    /// 64 points in an 8×8 grid about 9 px apart, 63×63 px in all: for a target that moves
    /// around the spot, such as a game character (ADR 0025).
    Area,
    /// Up to 64 offsets of your own.
    Custom,
}

/// The rows and columns of [`PatternKind::Area`]: within [`MAX_REACH`] of the anchor, symmetric.
const AREA_STEPS: [i16; 8] = [-31, -22, -13, -4, 4, 13, 22, 31];

impl PatternKind {
    /// The pixel offsets from the anchor. `plus_distance` is used by `Plus` only, `custom` by
    /// `Custom` only.
    pub fn points(self, plus_distance: u16, custom: &[[i16; 2]]) -> Vec<(i16, i16)> {
        let square = |r: i16| (-r..=r).flat_map(|y| (-r..=r).map(move |x| (x, y))).collect();
        match self {
            Self::Pixel => vec![(0, 0)],
            Self::Center2x2 => vec![(-1, -1), (0, -1), (-1, 0), (0, 0)],
            Self::Square3 => square(1),
            Self::Square5 => square(2),
            Self::Plus => {
                let d = i16::try_from(plus_distance).unwrap_or(i16::MAX);
                vec![(0, 0), (-d, 0), (d, 0), (0, -d), (0, d)]
            }
            Self::Area => AREA_STEPS
                .iter()
                .flat_map(|&y| AREA_STEPS.iter().map(move |&x| (x, y)))
                .collect(),
            Self::Custom => custom.iter().map(|&[x, y]| (x, y)).collect(),
        }
    }

    /// `k_enter` when the profile doesn't set it: the §5.3 table, and for a custom pattern a
    /// majority of its pixels.
    pub fn default_k_enter(self, pixels: usize) -> u8 {
        match self {
            Self::Pixel => 1,
            Self::Center2x2 => 3,
            Self::Square3 => 5,
            Self::Square5 => 13,
            Self::Plus => 4,
            // An eighth of the grid: part of a target the size of the area is enough.
            Self::Area => 8,
            Self::Custom => u8::try_from(pixels / 2 + 1).unwrap_or(u8::MAX),
        }
    }
}

/// `k_stay` when the profile doesn't set it: ⌈k_enter / 2⌉, which reproduces every row of the
/// §5.3 table (ADR 0006).
pub const fn derived_k_stay(k_enter: u8) -> u8 {
    k_enter.div_ceil(2)
}

/// A rectangle in X11 pixel coordinates.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: u16, h: u16) -> Self {
        Self { x, y, w, h }
    }

    /// The anchor pixel: `(x + ⌊w/2⌋, y + ⌊h/2⌋)`. For an odd size it is the center pixel. For
    /// an even size the true center lies between pixels, and this is the pixel just right of
    /// and below it, so [`PatternKind::Center2x2`] covers the four pixels around the center
    /// (§5.1, ADR 0006).
    pub fn anchor(&self) -> (i32, i32) {
        (self.x + i32::from(self.w / 2), self.y + i32::from(self.h / 2))
    }

    /// Whether the two rectangles share at least one pixel.
    pub fn intersects(&self, other: &Rect) -> bool {
        let (ax2, ay2) = (self.x + i32::from(self.w), self.y + i32::from(self.h));
        let (bx2, by2) = (other.x + i32::from(other.w), other.y + i32::from(other.h));
        self.x < bx2 && other.x < ax2 && self.y < by2 && other.y < ay2
    }

    pub fn contains(&self, other: &Rect) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.x + i32::from(other.w) <= self.x + i32::from(self.w)
            && other.y + i32::from(other.h) <= self.y + i32::from(self.h)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatternError {
    #[error("a pattern needs at least one pixel")]
    Empty,
    #[error("a pattern can have at most {MAX_POINTS} pixels, not {0}")]
    TooMany(usize),
    #[error("pixel ({0}, {1}) is listed twice")]
    Duplicate(i16, i16),
    #[error("pixel ({0}, {1}) is more than {MAX_REACH} px from the center")]
    TooFar(i16, i16),
    #[error("k_enter must be between 1 and {pixels} (the pattern's pixels), not {k}")]
    KEnter { k: u8, pixels: usize },
    #[error("k_stay must be between 1 and k_enter ({k_enter}), not {k}")]
    KStay { k: u8, k_enter: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the {area_w}×{area_h} area is smaller than the {w}×{h} sample pattern")]
pub struct TooSmall {
    pub area_w: u16,
    pub area_h: u16,
    pub w: u16,
    pub h: u16,
}

/// Where a pattern is captured inside a monitor or window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Placement {
    /// The rectangle to capture, in the area's coordinates.
    pub rect: Rect,
    /// The offset pushed the pattern past an edge, and it was moved back inside. Validation
    /// rejects such an offset; a target that later shrinks gets clamped (§5.1, §11).
    pub clamped: bool,
}

/// A pattern ready for the hot loop: the rectangle to capture around the anchor and which of
/// its pixels to count. It is `Copy` and fixed-size, so the hot loop never allocates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SamplePattern {
    /// The bounding rectangle, relative to the anchor.
    rect: Rect,
    /// Row-major indices of the pattern's pixels inside `rect`.
    idx: [u16; MAX_POINTS],
    n: u8,
    k_enter: u8,
    k_stay: u8,
}

/// Per class code (absent, wide, tight, excluded): a tight pixel also counts as wide. The
/// tight count goes in the high byte and the wide count in the low byte (at most 64 each).
const COUNT: [u16; 4] = [0x0000, 0x0001, 0x0101, 0x0000];

impl SamplePattern {
    pub fn new(points: &[(i16, i16)], k_enter: u8, k_stay: u8) -> Result<Self, PatternError> {
        if points.is_empty() {
            return Err(PatternError::Empty);
        }
        if points.len() > MAX_POINTS {
            return Err(PatternError::TooMany(points.len()));
        }
        for (i, &(x, y)) in points.iter().enumerate() {
            if x.unsigned_abs() > MAX_REACH || y.unsigned_abs() > MAX_REACH {
                return Err(PatternError::TooFar(x, y));
            }
            if points[..i].contains(&(x, y)) {
                return Err(PatternError::Duplicate(x, y));
            }
        }
        if k_enter == 0 || usize::from(k_enter) > points.len() {
            return Err(PatternError::KEnter {
                k: k_enter,
                pixels: points.len(),
            });
        }
        if k_stay == 0 || k_stay > k_enter {
            return Err(PatternError::KStay { k: k_stay, k_enter });
        }
        // The reach limit keeps every coordinate within ±31, so these conversions can't fail.
        let min_x = points.iter().map(|p| p.0).min().unwrap_or(0);
        let min_y = points.iter().map(|p| p.1).min().unwrap_or(0);
        let w = points.iter().map(|p| p.0).max().unwrap_or(0) - min_x + 1;
        let h = points.iter().map(|p| p.1).max().unwrap_or(0) - min_y + 1;
        let mut idx = [0u16; MAX_POINTS];
        for (slot, &(x, y)) in idx.iter_mut().zip(points) {
            *slot = (y - min_y).unsigned_abs() * w.unsigned_abs() + (x - min_x).unsigned_abs();
        }
        Ok(Self {
            rect: Rect::new(i32::from(min_x), i32::from(min_y), w.unsigned_abs(), h.unsigned_abs()),
            idx,
            n: points.len() as u8,
            k_enter,
            k_stay,
        })
    }

    /// A pattern from a profile's `[sampling]` fields; a missing `k_enter` or `k_stay` takes
    /// its default.
    pub fn from_profile(
        kind: PatternKind,
        plus_distance: Option<u16>,
        offsets: &[[i16; 2]],
        k_enter: Option<u8>,
        k_stay: Option<u8>,
    ) -> Result<Self, PatternError> {
        let points = kind.points(plus_distance.unwrap_or(DEFAULT_PLUS_DISTANCE), offsets);
        let k_enter = k_enter.unwrap_or_else(|| kind.default_k_enter(points.len()));
        Self::new(&points, k_enter, k_stay.unwrap_or(derived_k_stay(k_enter)))
    }

    /// How many pixels are counted.
    pub fn pixel_count(&self) -> usize {
        usize::from(self.n)
    }

    pub fn k_enter(&self) -> u8 {
        self.k_enter
    }

    pub fn k_stay(&self) -> u8 {
        self.k_stay
    }

    /// The width and height of the captured rectangle.
    pub fn size(&self) -> (u16, u16) {
        (self.rect.w, self.rect.h)
    }

    /// How many pixels a capture holds: `width × height`.
    pub fn area(&self) -> usize {
        usize::from(self.rect.w) * usize::from(self.rect.h)
    }

    /// The pattern's pixels as offsets from the anchor.
    pub fn points(&self) -> impl Iterator<Item = (i16, i16)> + '_ {
        let w = self.rect.w;
        self.idx[..self.pixel_count()]
            .iter()
            .map(move |&i| ((i % w) as i16 + self.rect.x as i16, (i / w) as i16 + self.rect.y as i16))
    }

    /// The capture rectangle for `area` (a monitor or the target window, §5.1) and `offset`
    /// from its anchor, moved back inside the area if the offset pushes it past an edge.
    pub fn place(&self, area: Rect, offset: (i32, i32)) -> Result<Placement, TooSmall> {
        let (w, h) = self.size();
        if area.w < w || area.h < h {
            return Err(TooSmall {
                area_w: area.w,
                area_h: area.h,
                w,
                h,
            });
        }
        let (ax, ay) = area.anchor();
        let want = Rect::new(ax + offset.0 + self.rect.x, ay + offset.1 + self.rect.y, w, h);
        let x = want.x.clamp(area.x, area.x + i32::from(area.w - w));
        let y = want.y.clamp(area.y, area.y + i32::from(area.h - h));
        Ok(Placement {
            rect: Rect::new(x, y, w, h),
            clamped: (x, y) != (want.x, want.y),
        })
    }

    /// Pixel agreement (§5.3): `pixels` is one capture of the pattern's rectangle, row-major,
    /// as `0xRRGGBB`.
    ///
    /// # Panics
    ///
    /// If `pixels` holds fewer than [`area`](Self::area) pixels.
    #[inline]
    pub fn verdict(&self, lut: &Lut, pixels: &[u32]) -> Verdict {
        let pixels = &pixels[..self.area()];
        let mut counts = 0u16;
        for &i in &self.idx[..usize::from(self.n)] {
            counts += COUNT[usize::from(lut.bits(pixels[usize::from(i)]))];
        }
        if (counts >> 8) as u8 >= self.k_enter {
            Verdict::Enter
        } else if (counts & 0xFF) as u8 >= self.k_stay {
            Verdict::Present
        } else {
            Verdict::Absent
        }
    }

    /// The anchor pixel of a capture, if the rectangle contains it (every built-in pattern
    /// does): the color the UI's live swatch shows.
    pub fn anchor_pixel(&self, pixels: &[u32]) -> Option<u32> {
        let (x, y) = (-self.rect.x, -self.rect.y);
        if x < 0 || y < 0 || x >= i32::from(self.rect.w) || y >= i32::from(self.rect.h) {
            return None;
        }
        pixels.get(y as usize * usize::from(self.rect.w) + x as usize).copied()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::LazyLock;

    use proptest::prelude::*;

    use super::*;
    use crate::color::{Class, ColorEntry, ColorModel, ColorRules, CompiledRules, Rgb};

    fn default_pattern(kind: PatternKind) -> SamplePattern {
        SamplePattern::from_profile(kind, None, &[], None, None).unwrap()
    }

    #[test]
    fn the_plan_table() {
        // §5.3: pixels, k_enter, k_stay. k_stay = ⌈k_enter / 2⌉ reproduces every row (ADR 0006).
        for (kind, pixels, k_enter, k_stay, size) in [
            (PatternKind::Pixel, 1, 1, 1, (1, 1)),
            (PatternKind::Center2x2, 4, 3, 2, (2, 2)),
            (PatternKind::Square3, 9, 5, 3, (3, 3)),
            (PatternKind::Square5, 25, 13, 7, (5, 5)),
            (PatternKind::Plus, 5, 4, 2, (5, 5)),
            // ADR 0025: the 8×8 grid fills MAX_POINTS and reaches MAX_REACH on every side.
            (PatternKind::Area, 64, 8, 4, (63, 63)),
        ] {
            let p = default_pattern(kind);
            assert_eq!(
                (p.pixel_count(), p.k_enter(), p.k_stay(), p.size()),
                (pixels, k_enter, k_stay, size),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn points_round_trip_through_the_rectangle() {
        for kind in [
            PatternKind::Pixel,
            PatternKind::Center2x2,
            PatternKind::Square3,
            PatternKind::Square5,
        ] {
            let p = default_pattern(kind);
            assert_eq!(
                p.points().collect::<Vec<_>>(),
                kind.points(DEFAULT_PLUS_DISTANCE, &[]),
                "{kind:?}"
            );
        }
        let p = SamplePattern::from_profile(PatternKind::Plus, Some(7), &[], None, None).unwrap();
        assert_eq!(
            p.points().collect::<Vec<_>>(),
            vec![(0, 0), (-7, 0), (7, 0), (0, -7), (0, 7)]
        );
        assert_eq!((p.size(), p.area()), ((15, 15), 225));
        let custom = [[3, -2], [-31, 31], [0, 0]];
        let p = SamplePattern::from_profile(PatternKind::Custom, None, &custom, None, None).unwrap();
        assert_eq!(p.points().collect::<Vec<_>>(), vec![(3, -2), (-31, 31), (0, 0)]);
        assert_eq!((p.k_enter(), p.k_stay()), (2, 1)); // majority of 3, then ⌈2/2⌉
    }

    #[test]
    fn invalid_patterns() {
        use PatternError::*;
        assert_eq!(SamplePattern::new(&[], 1, 1), Err(Empty));
        assert_eq!(
            SamplePattern::new(&[(0, 0), (1, 0), (0, 0)], 1, 1),
            Err(Duplicate(0, 0))
        );
        assert_eq!(SamplePattern::new(&[(32, 0)], 1, 1), Err(TooFar(32, 0)));
        assert_eq!(SamplePattern::new(&[(0, -32)], 1, 1), Err(TooFar(0, -32)));
        let many: Vec<_> = (0..65).map(|i| (i % 9 - 4, i / 9 - 4)).collect();
        assert_eq!(SamplePattern::new(&many, 1, 1), Err(TooMany(65)));
        assert!(SamplePattern::new(&many[..64], 1, 1).is_ok());
        let nine = PatternKind::Square3.points(0, &[]);
        assert_eq!(SamplePattern::new(&nine, 10, 1), Err(KEnter { k: 10, pixels: 9 }));
        assert_eq!(SamplePattern::new(&nine, 0, 0), Err(KEnter { k: 0, pixels: 9 }));
        assert_eq!(SamplePattern::new(&nine, 5, 6), Err(KStay { k: 6, k_enter: 5 }));
        assert_eq!(SamplePattern::new(&nine, 5, 0), Err(KStay { k: 0, k_enter: 5 }));
        assert!(SamplePattern::from_profile(PatternKind::Plus, Some(32), &[], None, None).is_err());
        assert_eq!(
            SamplePattern::from_profile(PatternKind::Custom, None, &[], None, None),
            Err(Empty)
        );
    }

    #[test]
    fn anchors_and_placement() {
        let monitor = Rect::new(0, 0, 1920, 1080);
        assert_eq!(monitor.anchor(), (960, 540));
        // The 2×2 block straddles the true center (960.0, 540.0) of an even-sized monitor.
        let p = default_pattern(PatternKind::Center2x2).place(monitor, (0, 0)).unwrap();
        assert_eq!(
            p,
            Placement {
                rect: Rect::new(959, 539, 2, 2),
                clamped: false
            }
        );
        let p = default_pattern(PatternKind::Square3).place(monitor, (0, 0)).unwrap();
        assert_eq!(p.rect, Rect::new(959, 539, 3, 3));
        // A second monitor to the right: its own center, never the seam between monitors.
        let right = Rect::new(1920, 0, 2560, 1440);
        assert_eq!(
            default_pattern(PatternKind::Pixel).place(right, (5, -3)).unwrap().rect,
            Rect::new(3205, 717, 1, 1)
        );
        // An odd-sized window: the exact center pixel.
        let window = Rect::new(10, 20, 101, 51);
        assert_eq!(window.anchor(), (60, 45));
        // An offset past the edge is clamped and reported; a window smaller than the pattern fails.
        let p = default_pattern(PatternKind::Square5).place(window, (60, 0)).unwrap();
        assert_eq!(
            p,
            Placement {
                rect: Rect::new(106, 43, 5, 5),
                clamped: true
            }
        );
        assert!(monitor.contains(&p.rect) && window.contains(&p.rect));
        let tiny = Rect::new(0, 0, 2, 2);
        assert_eq!(
            default_pattern(PatternKind::Square3).place(tiny, (0, 0)),
            Err(TooSmall {
                area_w: 2,
                area_h: 2,
                w: 3,
                h: 3
            })
        );
    }

    /// Exact colors: T is tight, W is in the wide band only, X is excluded, A is absent.
    const T: u32 = 0x40_80_C0;
    const W: u32 = 0x40_80_C3;
    const X: u32 = 0x10_10_10;
    const A: u32 = 0xFF_FF_FF;

    static LUT: LazyLock<Lut> = LazyLock::new(|| {
        let mut include = ColorEntry::new(Rgb::from_u32(T));
        include.tol_rgb = Some(2); // tight ±2, wide ±3
        let mut exclude = ColorEntry::new(Rgb::from_u32(X));
        exclude.tol_rgb = Some(0);
        let rules = ColorRules {
            model: ColorModel::Rgb,
            include: vec![include],
            exclude: vec![exclude],
            ..ColorRules::default()
        };
        let lut = Lut::build(&CompiledRules::new(&rules));
        assert_eq!(
            [T, W, X, A].map(|c| lut.class(c)),
            [Class::Tight, Class::Wide, Class::Excluded, Class::Absent]
        );
        lut
    });

    fn lut() -> &'static Lut {
        &LUT
    }

    #[test]
    fn pixel_agreement_on_a_3x3_patch() {
        let lut = lut();
        let p = default_pattern(PatternKind::Square3); // 5 to enter, 3 to stay
        let v = |px: [u32; 9]| p.verdict(lut, &px);
        assert_eq!(v([T; 9]), Verdict::Enter);
        assert_eq!(v([T, T, T, T, T, A, A, A, A]), Verdict::Enter);
        assert_eq!(v([T, T, T, T, A, A, A, A, A]), Verdict::Present); // 4 tight still count as wide
        assert_eq!(v([W, W, W, A, A, A, A, A, A]), Verdict::Present);
        assert_eq!(v([W, W, A, A, A, A, A, A, A]), Verdict::Absent);
        assert_eq!(v([W; 9]), Verdict::Present); // the wide band alone never enters
        assert_eq!(v([X; 9]), Verdict::Absent); // excluded pixels count for nothing
        assert_eq!(v([T, T, T, T, X, X, X, X, X]), Verdict::Present);
        // One odd pixel can neither make nor break an appearance.
        assert_eq!(v([T, T, T, T, A, T, T, T, T]), Verdict::Enter);
        assert_eq!(v([A, A, A, A, T, A, A, A, A]), Verdict::Absent);
    }

    #[test]
    fn sparse_patterns_ignore_pixels_outside_the_pattern() {
        let lut = lut();
        let p = SamplePattern::from_profile(PatternKind::Plus, Some(1), &[], None, None).unwrap(); // 4 to enter
        let mut px = [A; 9];
        for i in [1, 3, 4, 5] {
            px[i] = T; // center and three arms
        }
        assert_eq!(p.verdict(lut, &px), Verdict::Enter);
        px = [T, A, T, A, A, A, T, A, T]; // only the corners, which aren't part of the plus
        assert_eq!(p.verdict(lut, &px), Verdict::Absent);
        assert_eq!(p.anchor_pixel(&px), Some(A));
        let custom = SamplePattern::from_profile(PatternKind::Custom, None, &[[2, 2]], None, None).unwrap();
        assert_eq!(custom.anchor_pixel(&[T]), None);
    }

    proptest! {
        #[test]
        fn verdict_follows_the_counts(
            classes in prop::collection::vec(0usize..4, 25),
            k_enter in 1u8..=25,
            k_stay_frac in 0.0f64..=1.0,
        ) {
            let lut = lut();
            let colors = [A, W, T, X];
            let px: Vec<u32> = classes.iter().map(|&c| colors[c]).collect();
            let k_stay = 1 + (f64::from(k_enter - 1) * k_stay_frac) as u8;
            let p = SamplePattern::new(&PatternKind::Square5.points(0, &[]), k_enter, k_stay).unwrap();
            let tight = classes.iter().filter(|&&c| c == 2).count();
            let wide = classes.iter().filter(|&&c| c == 1 || c == 2).count();
            let expected = if tight >= usize::from(k_enter) {
                Verdict::Enter
            } else if wide >= usize::from(k_stay) {
                Verdict::Present
            } else {
                Verdict::Absent
            };
            prop_assert_eq!(p.verdict(lut, &px), expected);
            // Enter implies Present: with k_stay ≤ k_enter, enough tight pixels are enough wide ones.
            prop_assert!(expected != Verdict::Enter || wide >= usize::from(k_stay));
        }
    }
}
