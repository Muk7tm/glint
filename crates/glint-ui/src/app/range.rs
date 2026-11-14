// SPDX-License-Identifier: MIT
//! More options → COLOR RANGE (PLAN §5.4, §12.4, §12.9): the shade and hue strips, the other
//! colors that count or never count, the color to check, and recording the watched spot.

use glint_core::Rgb;
use glint_core::color::oklab::Oklch;
use glint_core::color::{Class, ColorModel, ShadePreset, Tolerances};
use glint_core::validate::{MAX_EXCLUDE, MAX_INCLUDE};
use glint_core::{Profile, color::ColorEntry};
use glint_ipc::{PaletteEntry, Request};
use glint_ui_view::{Cell, Chip, Form, Tone};
use slint::{ComponentHandle, ModelRc, VecModel};

use super::App;
use crate::bind;

/// Cells in a strip (§12.6).
const CELLS: usize = 16;
/// *Record 5 s*.
const RECORD_MS: u32 = 5000;

#[derive(Default)]
pub struct RangeState {
    /// The color in *Check hex*, once it is one.
    check: Option<Rgb>,
    /// The next *Sample center* goes to *Check hex*, not to the main color.
    pub sample_to_check: bool,
    /// The last recording's colors, most frequent first.
    recorded: Vec<Rgb>,
    /// The colors last sent to be classified for the strips: shades, then hues.
    strip_colors: Vec<Rgb>,
    strips: [Vec<Cell>; 2],
}

/// The strips around `main` (§12.4): 16 shades (lightness) and 16 hues, spanning a little more
/// than the range `tol` allows, so both of its ends show.
pub fn strip_colors(main: Rgb, tol: Tolerances) -> Vec<Rgb> {
    let lch = Oklch::from_rgb(main);
    let (dl, dh) = match tol {
        Tolerances::Oklch { l, h, .. } => (l, h),
        _ => (0.10, 7.0),
    };
    let (span_l, span_h) = ((dl * 2.2).max(0.12), (dh * 2.2).max(24.0));
    let at = |k: usize| (k as f64 - 7.5) / 7.5;
    let shades = (0..CELLS).map(|k| {
        Oklch {
            l: (lch.l + at(k) * span_l).clamp(0.02, 0.98),
            ..lch
        }
        .to_rgb()
    });
    let hues = (0..CELLS).map(|k| {
        Oklch {
            h: (lch.h + at(k) * span_h).rem_euclid(360.0),
            ..lch
        }
        .to_rgb()
    });
    shades.chain(hues).collect()
}

/// How a cell is drawn: 2 starts an appearance (full color), 1 keeps one going, 0 neither.
fn level(class: Class) -> i32 {
    match class {
        Class::Tight => 2,
        Class::Wide => 1,
        Class::Absent | Class::Excluded => 0,
    }
}

/// What *Check hex* says about a color.
pub fn check_text(class: Class) -> (&'static str, Tone) {
    match class {
        Class::Tight => ("Counts: it starts an appearance.", Tone::Success),
        Class::Wide => (
            "Close: it keeps an appearance going, but doesn't start one.",
            Tone::Neutral,
        ),
        Class::Absent => ("Doesn't count.", Tone::Neutral),
        Class::Excluded => ("Never matches: it's excluded.", Tone::Warning),
    }
}

fn color(rgb: Rgb) -> slint::Color {
    slint::Color::from_rgb_u8(rgb.r(), rgb.g(), rgb.b())
}

fn chips(colors: impl Iterator<Item = Rgb>) -> ModelRc<Chip> {
    ModelRc::new(VecModel::from(
        colors
            .map(|rgb| Chip {
                color: color(rgb),
                hex: rgb.to_string().into(),
            })
            .collect::<Vec<_>>(),
    ))
}

impl App {
    /// After the profile changed: the strips, and the checked color, need the engine's word.
    pub(super) fn classify_range(&mut self) {
        match self.profile.color.include.first() {
            Some(e) => {
                let colors = strip_colors(e.hex, e.tolerances(ColorModel::Oklch, ShadePreset::Normal));
                self.range.strip_colors.clone_from(&colors);
                self.link.send(Request::Classify { colors });
            }
            None => {
                self.range.strip_colors.clear();
                self.range.strips = Default::default();
            }
        }
        if let Some(rgb) = self.range.check {
            self.link.send(Request::Classify { colors: vec![rgb] });
        }
    }

    /// The engine's verdicts: the strips' 32, or the checked color's one.
    pub(super) fn classified(&mut self, verdicts: &[Class]) {
        if verdicts.len() == 2 * CELLS && self.range.strip_colors.len() == 2 * CELLS {
            let cells: Vec<Cell> = self
                .range
                .strip_colors
                .iter()
                .zip(verdicts)
                .map(|(&rgb, &class)| Cell {
                    color: color(rgb),
                    level: level(class),
                })
                .collect();
            self.range.strips = [cells[..CELLS].to_vec(), cells[CELLS..].to_vec()];
            self.show_range();
        } else if let ([class], Some(_)) = (verdicts, self.range.check)
            && let Some(ui) = self.ui.upgrade()
        {
            let (text, tone) = check_text(*class);
            let f = ui.global::<Form>();
            f.set_check_text(text.into());
            f.set_check_tone(tone);
        }
    }

    /// Shows the lists, the strips and the recording.
    pub(super) fn show_range(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let f = ui.global::<Form>();
        f.set_also_match(chips(self.profile.color.include.iter().skip(1).map(|e| e.hex)));
        f.set_never_match(chips(self.profile.color.exclude.iter().map(|e| e.hex)));
        f.set_shade_strip(ModelRc::new(VecModel::from(self.range.strips[0].clone())));
        f.set_hue_strip(ModelRc::new(VecModel::from(self.range.strips[1].clone())));
        f.set_recorded(chips(self.range.recorded.iter().copied()));
        f.set_sample_key(self.settings.hotkeys.sample_center.as_str().into());
    }

    /// A typed hex code in *Check hex*.
    pub fn check_hex(&mut self, text: &str) {
        match crate::keys::parse_hex(text) {
            Some(rgb) => self.set_check(rgb),
            None => {
                self.range.check = None;
                if let Some(ui) = self.ui.upgrade() {
                    let f = ui.global::<Form>();
                    f.set_check_known(false);
                    f.set_check_text("Not a color code: use #RRGGBB.".into());
                    f.set_check_tone(Tone::Error);
                }
            }
        }
    }

    /// Puts `rgb` in *Check hex* and asks the engine what it counts as.
    pub(super) fn set_check(&mut self, rgb: Rgb) {
        self.range.check = Some(rgb);
        if let Some(ui) = self.ui.upgrade() {
            let f = ui.global::<Form>();
            f.set_check_known(true);
            f.set_check_color(color(rgb));
            f.set_check_hex(rgb.to_string().into());
            f.set_check_text("".into());
        }
        self.link.send(Request::Classify { colors: vec![rgb] });
    }

    /// *Also match*: the checked color counts too, with the main color's shades.
    pub fn add_also(&mut self) {
        let Some(rgb) = self.range.check else { return };
        let mut p = self.profile.clone();
        if p.color.include.iter().any(|e| e.hex == rgb) {
            return self.toast(&format!("{rgb} already counts."), Tone::Neutral);
        }
        if p.color.include.len() >= MAX_INCLUDE {
            return self.toast(&format!("At most {MAX_INCLUDE} colors."), Tone::Warning);
        }
        let entry = bind::shade_preset(self.shown.shades).entry(p.color.model, rgb);
        p.color.include.push(entry);
        self.send_profile(p);
    }

    /// *Never match*: the checked color never counts, with a tight range of its own (§5.4).
    pub fn add_never(&mut self) {
        let Some(rgb) = self.range.check else { return };
        let mut p = self.profile.clone();
        if p.color.exclude.iter().any(|e| e.hex == rgb) {
            return self.toast(&format!("{rgb} already never matches."), Tone::Neutral);
        }
        if p.color.exclude.len() >= MAX_EXCLUDE {
            return self.toast(
                &format!("At most {MAX_EXCLUDE} colors that never match."),
                Tone::Warning,
            );
        }
        let entry: ColorEntry = ShadePreset::Tight.entry(p.color.model, rgb);
        p.color.exclude.push(entry);
        self.send_profile(p);
    }

    pub fn remove_also(&mut self, i: i32) {
        let mut p = self.profile.clone();
        if let Some(i) = usize::try_from(i).ok().map(|i| i + 1)
            && i < p.color.include.len()
        {
            p.color.include.remove(i);
            self.send_profile(p);
        }
    }

    pub fn remove_never(&mut self, i: i32) {
        let mut p = self.profile.clone();
        if let Some(i) = usize::try_from(i).ok()
            && i < p.color.exclude.len()
        {
            p.color.exclude.remove(i);
            self.send_profile(p);
        }
    }

    /// Sends `p` as the profile, shows it, and asks for the strips again.
    pub(super) fn send_profile(&mut self, p: Profile) {
        self.put_profile(p.clone());
        self.profile = p;
        self.refresh();
        self.classify_range();
    }

    /// *Record 5 s* (§12.9).
    pub fn record(&mut self) {
        self.link.send(Request::RecordPalette { ms: RECORD_MS });
        self.set_recording(true);
    }

    pub(super) fn set_recording(&self, on: bool) {
        if let Some(ui) = self.ui.upgrade() {
            ui.global::<Form>().set_recording(on);
        }
    }

    /// A recording's colors: click one to check it.
    pub(super) fn recorded(&mut self, entries: &[PaletteEntry]) {
        self.range.recorded = entries.iter().map(|e| e.rgb).collect();
        self.set_recording(false);
        self.show_range();
        if self.range.recorded.is_empty() {
            self.toast("Nothing was recorded.", Tone::Warning);
        }
    }

    pub fn choose_recorded(&mut self, i: i32) {
        if let Some(&rgb) = usize::try_from(i).ok().and_then(|i| self.range.recorded.get(i)) {
            self.set_check(rgb);
        }
    }

    /// *Sample center* from the card: the color goes to *Check hex*.
    pub fn sample_for_check(&mut self) {
        self.range.sample_to_check = true;
        self.link.send(Request::SampleCenter);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strips_span_a_little_more_than_the_range() {
        let purple = Rgb::from_u32(0x86_04_A0);
        let tol = Tolerances::Oklch {
            l: 0.16,
            c: 0.08,
            h: 12.0,
        };
        let colors = strip_colors(purple, tol);
        assert_eq!(colors.len(), 32);
        let base = Oklch::from_rgb(purple);
        // The shades run dark to light around the color; the hues keep its lightness.
        let first = Oklch::from_rgb(colors[0]);
        let last = Oklch::from_rgb(colors[15]);
        assert!(first.l < base.l - 0.16 && last.l > base.l + 0.16, "{first:?} {last:?}");
        let hue_ends = (Oklch::from_rgb(colors[16]).h, Oklch::from_rgb(colors[31]).h);
        let d = |h: f64| glint_core::color::oklab::hue_distance(h, base.h);
        assert!(d(hue_ends.0) > 12.0 && d(hue_ends.1) > 12.0, "{hue_ends:?}");
    }

    #[test]
    fn what_check_hex_says() {
        assert_eq!(check_text(Class::Tight).1, Tone::Success);
        assert_eq!(check_text(Class::Excluded).1, Tone::Warning);
        assert_eq!(level(Class::Wide), 1);
    }
}
