// SPDX-License-Identifier: MIT
//! Between the engine's data and the window (PLAN §12): what the window shows for a profile, the
//! settings and the engine's state ([`fields`], [`hero`]), and the profile and settings its edits
//! make ([`edited`]). Plain data, tested without a window; [`apply`] copies it into the Slint
//! globals.

use glint_core::color::{ColorModel, ShadePreset, Tolerances};
use glint_core::pattern::PatternKind;
use glint_core::profile::{ActionKind, Backend, MouseButton, Speed, TargetMode};
use glint_core::settings::{ClosePolicy, Delivery, HotkeyMode, Settings};
use glint_core::{Profile, Rgb};
use glint_ui_view::{AppWindow, Form, Ui};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

/// The *Click* dropdown's mouse entries, in order; the next entry is "Keyboard key".
const BUTTONS: [MouseButton; 5] = [
    MouseButton::Left,
    MouseButton::Right,
    MouseButton::Middle,
    MouseButton::Back,
    MouseButton::Forward,
];
pub const KEYBOARD_KEY: i32 = BUTTONS.len() as i32;
const SHADES: [ShadePreset; 4] = [
    ShadePreset::Tight,
    ShadePreset::Normal,
    ShadePreset::Loose,
    ShadePreset::LoosePlus,
];
const CLOSING: [ClosePolicy; 3] = [ClosePolicy::Ask, ClosePolicy::KeepRunning, ClosePolicy::Quit];
/// *Pattern* (§12.4); a custom pattern selects none.
const PATTERNS: [PatternKind; 5] = [
    PatternKind::Pixel,
    PatternKind::Center2x2,
    PatternKind::Square3,
    PatternKind::Square5,
    PatternKind::Plus,
];
/// *Checks*; a number of checks per second selects none.
const SPEEDS: [Speed; 3] = [Speed::Eco, Speed::Balanced, Speed::Max];
/// *Read from*: Auto, Screen, Shared memory, Window buffer, Damage.
const BACKENDS: [Backend; 5] = [
    Backend::Auto,
    Backend::Screen,
    Backend::ScreenShm,
    Backend::WindowBuffer,
    Backend::Damage,
];

/// *Model* (§5.4).
const MODELS: [ColorModel; 3] = [ColorModel::Oklch, ColorModel::Hsv, ColorModel::Rgb];

/// More options → COLOR RANGE (§12.4): the model, and the main color's tolerances as the window
/// shows them: OKLCh lightness ×100, chroma ×100, hue in degrees; HSV hue in degrees, saturation
/// ×100, value ×100; RGB one value per channel.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Range {
    pub model: i32,
    pub a: i32,
    pub b: i32,
    pub c: i32,
}

fn range(profile: &Profile) -> Range {
    let model = profile.color.model;
    let pct = |v: f64| (v * 100.0).round() as i32;
    let (a, b, c) =
        profile
            .color
            .include
            .first()
            .map_or((0, 0, 0), |e| match e.tolerances(model, ShadePreset::Normal) {
                Tolerances::Oklch { l, c, h } => (pct(l), pct(c), h.round() as i32),
                Tolerances::Hsv { h, s, v } => (h.round() as i32, pct(s), pct(v)),
                Tolerances::Rgb { t } => (t.round() as i32, 0, 0),
            });
    Range {
        model: index_of(&MODELS, &model).max(0),
        a,
        b,
        c,
    }
}

/// The profile with the window's COLOR RANGE edits: a new model, or the main color's changed
/// tolerances (only those that changed, so the others keep their exact values).
fn edit_range(p: &mut Profile, r: &Range, before: &Range) {
    if r.model != before.model {
        if let Some(model) = pick(&MODELS, r.model) {
            p.color.model = model;
        }
        return;
    }
    let model = p.color.model;
    let Some(e) = p.color.include.first_mut() else { return };
    let (a, b, c) = (r.a != before.a, r.b != before.b, r.c != before.c);
    let pct = |v: i32| f64::from(v.max(0)) / 100.0;
    match model {
        ColorModel::Oklch => {
            if a {
                e.tol_l = Some(pct(r.a));
            }
            if b {
                e.tol_c = Some(pct(r.b));
            }
            if c {
                e.tol_h = Some(f64::from(r.c.max(0)));
            }
        }
        ColorModel::Hsv => {
            if a {
                e.tol_h = Some(f64::from(r.a.max(0)));
            }
            if b {
                e.tol_s = Some(pct(r.b));
            }
            if c {
                e.tol_v = Some(pct(r.c));
            }
        }
        ColorModel::Rgb => {
            if a {
                e.tol_rgb = u8::try_from(r.a.clamp(0, 255)).ok();
            }
        }
    }
}

/// More options → DETECTION (§12.4).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Detection {
    /// An index into [`PATTERNS`]; -1 for a custom pattern.
    pub pattern: i32,
    /// *Must match*: `k_enter`, of `pixels`.
    pub must_match: i32,
    pub pixels: i32,
    /// An index into [`SPEEDS`]; -1 for a number of checks per second.
    pub speed: i32,
    pub speed_text: String,
    /// *Gone after*, *Min gap*, *Safety stop*.
    pub gone_after: i32,
    pub min_gap: i32,
    pub safety_stop: i32,
    /// An index into [`BACKENDS`].
    pub backend: i32,
    pub offset_x: i32,
    pub offset_y: i32,
}

/// `n` with a thousands separator, the way the window writes counts.
pub fn thousands(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn detection(profile: &Profile) -> Detection {
    let (s, t) = (&profile.sampling, &profile.trigger);
    let (must_match, pixels) = profile
        .pattern()
        .map_or((1, 1), |p| (i32::from(p.k_enter()), p.pixel_count() as i32));
    Detection {
        pattern: index_of(&PATTERNS, &s.pattern),
        must_match,
        pixels,
        speed: index_of(&SPEEDS, &s.speed),
        speed_text: match s.speed.checks_per_second() {
            Some(n) => format!("{}/sec", thousands(n)),
            None => "as fast as possible".into(),
        },
        gone_after: t.rearm_absent_ms as i32,
        min_gap: t.min_fire_interval_ms as i32,
        safety_stop: t.max_clicks_per_sec as i32,
        backend: index_of(&BACKENDS, &s.backend).max(0),
        offset_x: profile.target.offset.x,
        offset_y: profile.target.offset.y,
    }
}

/// The item at index `i`, if there is one.
fn pick<T: Copy>(items: &[T], i: i32) -> Option<T> {
    usize::try_from(i).ok().and_then(|i| items.get(i).copied())
}

/// The profile with the window's DETECTION edits `d` (`before` is what it showed).
fn edit_detection(p: &mut Profile, d: &Detection, before: &Detection) {
    if d.pattern != before.pattern {
        if let Some(kind) = pick(&PATTERNS, d.pattern) {
            // A new pattern starts from its own *Must match*.
            p.sampling.pattern = kind;
            p.sampling.k_enter = None;
            p.sampling.k_stay = None;
        }
    } else if d.must_match != before.must_match {
        p.sampling.k_enter = u8::try_from(d.must_match.max(1)).ok();
        p.sampling.k_stay = None;
    }
    if d.speed != before.speed
        && let Some(speed) = pick(&SPEEDS, d.speed)
    {
        p.sampling.speed = speed;
    }
    if d.backend != before.backend
        && let Some(backend) = pick(&BACKENDS, d.backend)
    {
        p.sampling.backend = backend;
    }
    let t = &mut p.trigger;
    if d.gone_after != before.gone_after {
        t.rearm_absent_ms = d.gone_after.clamp(0, 2000) as u32;
    }
    if d.min_gap != before.min_gap {
        t.min_fire_interval_ms = d.min_gap.clamp(0, 1000) as u32;
    }
    if d.safety_stop != before.safety_stop {
        t.max_clicks_per_sec = d.safety_stop.clamp(1, 100) as u32;
    }
    if d.offset_x != before.offset_x {
        p.target.offset.x = d.offset_x;
    }
    if d.offset_y != before.offset_y {
        p.target.offset.y = d.offset_y;
    }
}

/// The editable values the window shows.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Fields {
    pub color: Option<Rgb>,
    /// Tight, Normal, Loose, Loose+; -1 when the range was set by hand (More options).
    pub shades: i32,
    /// Screen center, window center.
    pub watch: i32,
    /// An index into [`BUTTONS`], or [`KEYBOARD_KEY`].
    pub click: i32,
    pub key_name: String,
    pub hold_ms: i32,
    /// Press to toggle, hold to start.
    pub mode: i32,
    pub hotkey: String,
    pub emergency: String,
    pub window_name: String,
    pub only_in_window: bool,
    /// Capture mode: the game doesn't get the hotkeys (§9).
    pub capture: bool,
    /// A start clicks a color already showing (`fire_if_present_on_start`, ADR 0023).
    pub fire_on_start: bool,
    /// Ask, keep running, quit.
    pub closing: i32,
    pub detection: Detection,
    pub range: Range,
}

/// The hero card (§12.3, §12.7).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Hero {
    pub button: String,
    pub enabled: bool,
    pub danger: bool,
    pub hint: String,
}

/// The shade preset at the *Shades* index, Normal for a range set by hand.
pub fn shade_preset(index: i32) -> ShadePreset {
    usize::try_from(index)
        .ok()
        .and_then(|i| SHADES.get(i))
        .copied()
        .unwrap_or(ShadePreset::Normal)
}

fn index_of<T: PartialEq>(items: &[T], item: &T) -> i32 {
    items.iter().position(|i| i == item).map_or(-1, |i| i as i32)
}

/// What the window shows for `profile` and `settings`.
pub fn fields(profile: &Profile, settings: &Settings) -> Fields {
    let first = profile.color.include.first();
    let model = profile.color.model;
    let shades = first.map_or(1, |e| {
        SHADES
            .iter()
            .position(|p| p.entry(model, e.hex) == *e)
            .map_or(-1, |i| i as i32)
    });
    let a = &profile.action;
    let window = &profile.target.window;
    Fields {
        color: first.map(|e| e.hex),
        shades,
        watch: i32::from(profile.target.mode == TargetMode::Window),
        click: match a.kind {
            ActionKind::Key => KEYBOARD_KEY,
            ActionKind::Mouse => index_of(&BUTTONS, &a.button).max(0),
        },
        key_name: a.key.as_ref().map(|k| k.name.clone()).unwrap_or_default(),
        hold_ms: a.hold_ms as i32,
        mode: i32::from(settings.hotkeys.mode == HotkeyMode::Hold),
        hotkey: settings.hotkeys.start_stop.clone(),
        emergency: settings.hotkeys.emergency_stop.clone(),
        window_name: if window.title_contains.is_empty() {
            window.class.clone()
        } else {
            window.title_contains.clone()
        },
        only_in_window: profile.guards.only_in_window,
        capture: settings.hotkeys.delivery == Delivery::Capture,
        fire_on_start: profile.trigger.fire_if_present_on_start,
        closing: index_of(&CLOSING, &settings.close_policy).max(0),
        detection: detection(profile),
        range: range(profile),
    }
}

/// `profile` and `settings` with the window's edits in `f`. A choice the window can't express
/// (a range set by hand, an unchanged field) keeps what the profile had.
pub fn edited(profile: &Profile, settings: &Settings, f: &Fields) -> (Profile, Settings) {
    let (mut p, mut s) = (profile.clone(), settings.clone());
    let before = fields(profile, settings);
    if f.shades != before.shades
        && let (Some(hex), Some(preset)) = (f.color, usize::try_from(f.shades).ok().and_then(|i| SHADES.get(i)))
    {
        let entry = preset.entry(p.color.model, hex);
        match p.color.include.first_mut() {
            Some(first) => *first = entry,
            None => p.color.include.push(entry),
        }
    }
    p.target.mode = if f.watch == 1 {
        TargetMode::Window
    } else {
        TargetMode::Screen
    };
    match usize::try_from(f.click).ok().and_then(|i| BUTTONS.get(i)) {
        Some(&button) => {
            p.action.kind = ActionKind::Mouse;
            p.action.button = button;
        }
        None => p.action.kind = ActionKind::Key,
    }
    p.action.hold_ms = f.hold_ms.clamp(0, 200) as u32;
    edit_detection(&mut p, &f.detection, &before.detection);
    edit_range(&mut p, &f.range, &before.range);
    p.guards.only_in_window = f.only_in_window;
    s.hotkeys.mode = if f.mode == 1 {
        HotkeyMode::Hold
    } else {
        HotkeyMode::Toggle
    };
    s.hotkeys.delivery = if f.capture { Delivery::Capture } else { Delivery::Listen };
    p.trigger.fire_if_present_on_start = f.fire_on_start;
    s.close_policy = usize::try_from(f.closing)
        .ok()
        .and_then(|i| CLOSING.get(i))
        .copied()
        .unwrap_or_default();
    (p, s)
}

/// The hero card for the engine's state: `started`, and why a Start would be refused.
pub fn hero(f: &Fields, started: bool, cannot_start: Option<&str>) -> Hero {
    let place = if f.window_name.is_empty() {
        "the screen".to_owned()
    } else {
        f.window_name.clone()
    };
    if started {
        return Hero {
            button: "■  Stop".into(),
            enabled: true,
            danger: true,
            hint: format!("Watching the center of {place} · {} stops everything", f.emergency),
        };
    }
    let blocked = match cannot_start {
        Some(why) => Some(why.to_owned()),
        None if f.color.is_none() => Some("Pick a color to start".into()),
        None => None,
    };
    // Hold mode watches only while the key is held (ADR 0013).
    let how = if f.mode == 1 {
        format!("hold {} to watch", f.hotkey)
    } else {
        format!("press {} to start", f.hotkey)
    };
    let hint = blocked.clone().unwrap_or_else(|| {
        if f.window_name.is_empty() {
            let mut how = how.clone();
            how[..1].make_ascii_uppercase();
            format!("{how} · {} stops everything", f.emergency)
        } else {
            format!(
                "Switch to {} and {how} · {} stops everything",
                f.window_name, f.emergency
            )
        }
    });
    Hero {
        button: "▶  Start".into(),
        enabled: blocked.is_none(),
        danger: false,
        hint,
    }
}

fn names(list: &[String]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(list.iter().map(SharedString::from).collect::<Vec<_>>()))
}

fn color(rgb: Rgb) -> slint::Color {
    slint::Color::from_rgb_u8(rgb.r(), rgb.g(), rgb.b())
}

/// Shows `f` and `hero` in the window.
pub fn apply(ui: &AppWindow, f: &Fields, hero: &Hero) {
    let form = ui.global::<Form>();
    form.set_has_color(f.color.is_some());
    if let Some(c) = f.color {
        form.set_color(color(c));
    }
    form.set_color_hex(f.color.map(|c| c.to_string()).unwrap_or_default().into());
    form.set_shades_index(f.shades);
    form.set_watch_index(f.watch);
    form.set_click_index(f.click);
    let keys = crate::keys::names();
    form.set_key_names(names(&keys));
    form.set_key_name(f.key_name.as_str().into());
    form.set_key_index(index_of(&keys, &f.key_name));
    form.set_hold_ms(f.hold_ms);
    form.set_mode_index(f.mode);
    let hotkeys = crate::keys::hotkey_names();
    form.set_hotkey_names(names(&hotkeys));
    form.set_hotkey(f.hotkey.as_str().into());
    form.set_hotkey_index(index_of(&hotkeys, &f.hotkey));
    form.set_emergency(f.emergency.as_str().into());
    form.set_window_name(f.window_name.as_str().into());
    form.set_only_in_window(f.only_in_window);
    form.set_capture_hotkeys(f.capture);
    form.set_fire_on_start(f.fire_on_start);
    form.set_closing_index(f.closing);
    let d = &f.detection;
    form.set_pattern_index(d.pattern);
    form.set_must_match(d.must_match);
    form.set_pattern_pixels(d.pixels);
    form.set_speed_index(d.speed);
    form.set_speed_text(d.speed_text.as_str().into());
    form.set_gone_after(d.gone_after);
    form.set_min_gap(d.min_gap);
    form.set_safety_stop(d.safety_stop);
    form.set_backend_index(d.backend);
    form.set_offset_x(d.offset_x);
    form.set_offset_y(d.offset_y);
    let r = &f.range;
    form.set_model_index(r.model);
    form.set_tol_a(r.a);
    form.set_tol_b(r.b);
    form.set_tol_c(r.c);
    let (labels, units, max, shown_b) = match pick(&MODELS, r.model).unwrap_or_default() {
        ColorModel::Oklch => (
            ["Lightness ±", "Colorfulness ±", "Hue ±"],
            ["", "degrees"],
            [100, 50, 180],
            true,
        ),
        ColorModel::Hsv => (
            ["Hue ±", "Saturation ±", "Value ±"],
            ["degrees", ""],
            [180, 100, 100],
            true,
        ),
        ColorModel::Rgb => (["Each channel ±", "", ""], ["of 255", ""], [255, 0, 0], false),
    };
    form.set_tol_a_label(labels[0].into());
    form.set_tol_b_label(labels[1].into());
    form.set_tol_c_label(labels[2].into());
    form.set_tol_a_unit(units[0].into());
    form.set_tol_c_unit(units[1].into());
    form.set_tol_a_max(max[0]);
    form.set_tol_b_max(max[1]);
    form.set_tol_c_max(max[2]);
    form.set_tol_b_shown(shown_b);

    let ui_state = ui.global::<Ui>();
    ui_state.set_button_text(hero.button.as_str().into());
    ui_state.set_button_enabled(hero.enabled);
    ui_state.set_button_danger(hero.danger);
    ui_state.set_hint(hero.hint.as_str().into());
    ui_state.set_start_key(f.hotkey.as_str().into());
    ui_state.set_stop_key(f.emergency.as_str().into());
}

/// The editable values as the window holds them now.
pub fn read(ui: &AppWindow, shown: &Fields) -> Fields {
    let form = ui.global::<Form>();
    Fields {
        shades: form.get_shades_index(),
        watch: form.get_watch_index(),
        click: form.get_click_index(),
        hold_ms: form.get_hold_ms(),
        mode: form.get_mode_index(),
        only_in_window: form.get_only_in_window(),
        capture: form.get_capture_hotkeys(),
        fire_on_start: form.get_fire_on_start(),
        closing: form.get_closing_index(),
        detection: Detection {
            pattern: form.get_pattern_index(),
            must_match: form.get_must_match(),
            speed: form.get_speed_index(),
            gone_after: form.get_gone_after(),
            min_gap: form.get_min_gap(),
            safety_stop: form.get_safety_stop(),
            backend: form.get_backend_index(),
            offset_x: form.get_offset_x(),
            offset_y: form.get_offset_y(),
            ..shown.detection.clone()
        },
        range: Range {
            model: form.get_model_index(),
            a: form.get_tol_a(),
            b: form.get_tol_b(),
            c: form.get_tol_c(),
        },
        ..shown.clone()
    }
}

#[cfg(test)]
mod tests {
    use glint_core::color::ColorModel;
    use glint_core::profile::KeySpec;

    use super::*;

    fn green() -> Profile {
        let mut p = Profile::new("Sober");
        p.color.include = vec![ShadePreset::Normal.entry(ColorModel::Oklch, Rgb::from_u32(0x3B_D1_6F))];
        p.target.window.title_contains = "Sober".into();
        p
    }

    #[test]
    fn a_profile_as_the_window_shows_it() {
        let f = fields(&green(), &Settings::default());
        assert_eq!(f.color, Some(Rgb::from_u32(0x3B_D1_6F)));
        assert_eq!((f.shades, f.watch, f.click, f.hold_ms, f.mode), (1, 0, 0, 10, 0));
        assert_eq!((f.hotkey.as_str(), f.emergency.as_str()), ("F7", "F8"));
        assert_eq!(
            (f.window_name.as_str(), f.only_in_window, f.closing),
            ("Sober", true, 0)
        );
        // A range set by hand matches no preset.
        let mut custom = green();
        custom.color.include[0].tol_l = Some(0.123);
        assert_eq!(fields(&custom, &Settings::default()).shades, -1);
        // A key action.
        let mut key = green();
        key.action.kind = ActionKind::Key;
        key.action.key = Some(KeySpec {
            keycode: 38,
            name: "a".into(),
        });
        let f = fields(&key, &Settings::default());
        assert_eq!((f.click, f.key_name.as_str()), (KEYBOARD_KEY, "a"));
    }

    #[test]
    fn edits_round_trip_and_keep_what_the_window_cant_show() {
        let (p, s) = (green(), Settings::default());
        let mut f = fields(&p, &s);
        f.shades = 2;
        f.click = 1;
        f.hold_ms = 50;
        f.mode = 1;
        f.only_in_window = false;
        f.closing = 2;
        f.fire_on_start = true;
        let (p2, s2) = edited(&p, &s, &f);
        // Loose shades show Loose's tolerances in COLOR RANGE.
        let want = Fields {
            range: Range {
                model: 0,
                a: 16,
                b: 8,
                c: 12,
            },
            ..f.clone()
        };
        assert_eq!(fields(&p2, &s2), want);
        assert_eq!(
            p2.color.include[0],
            ShadePreset::Loose.entry(ColorModel::Oklch, Rgb::from_u32(0x3B_D1_6F))
        );
        assert_eq!(p2.action.button, MouseButton::Right);
        assert!(p2.trigger.fire_if_present_on_start);
        assert_eq!(
            (s2.hotkeys.mode, s2.close_policy),
            (HotkeyMode::Hold, ClosePolicy::Quit)
        );
        // Untouched shades keep a range set by hand.
        let mut custom = green();
        custom.color.include[0].tol_l = Some(0.123);
        let f = fields(&custom, &s);
        assert_eq!(edited(&custom, &s, &f).0, custom);
        // The hold stays in its range.
        let mut f = fields(&p, &s);
        f.hold_ms = 900;
        assert_eq!(edited(&p, &s, &f).0.action.hold_ms, 200);
    }

    #[test]
    fn detection_shows_and_edits_the_profile() {
        let s = Settings::default();
        let p = green();
        let f = fields(&p, &s);
        let d = &f.detection;
        assert_eq!((d.pattern, d.must_match, d.pixels), (2, 5, 9));
        assert_eq!((d.speed, d.speed_text.as_str()), (1, "2,000/sec"));
        assert_eq!((d.gone_after, d.min_gap, d.safety_stop, d.backend), (40, 25, 15, 0));
        // A new pattern starts from its own Must match.
        let mut g = f.clone();
        g.detection.pattern = 3;
        g.detection.must_match = 2;
        let (p2, _) = edited(&p, &s, &g);
        assert_eq!((p2.sampling.pattern, p2.sampling.k_enter), (PatternKind::Square5, None));
        // Then Must match, speed, Gone after, Read from and the offset.
        let mut g = fields(&p2, &s);
        g.detection.must_match = 7;
        g.detection.speed = 2;
        g.detection.gone_after = 120;
        g.detection.backend = 4;
        g.detection.offset_x = -12;
        let (p3, _) = edited(&p2, &s, &g);
        assert_eq!(p3.sampling.k_enter, Some(7));
        assert_eq!((p3.sampling.speed, p3.sampling.backend), (Speed::Max, Backend::Damage));
        assert_eq!((p3.trigger.rearm_absent_ms, p3.target.offset.x), (120, -12));
        assert_eq!(fields(&p3, &s).detection.speed_text, "as fast as possible");
        // Untouched values stay as they were.
        assert_eq!(p3.trigger.min_fire_interval_ms, p.trigger.min_fire_interval_ms);
        assert_eq!(thousands(500), "500");
        assert_eq!(thousands(10_000), "10,000");
    }

    #[test]
    fn color_range_edits_one_tolerance_at_a_time() {
        let s = Settings::default();
        let mut p = green();
        p.color.include[0].tol_l = Some(0.165);
        let mut f = fields(&p, &s);
        assert_eq!((f.range.a, f.range.b, f.range.c), (17, 5, 7));
        // Hue only: lightness keeps its exact 0.165.
        f.range.c = 20;
        let (p2, _) = edited(&p, &s, &f);
        let e = &p2.color.include[0];
        assert_eq!((e.tol_l, e.tol_h), (Some(0.165), Some(20.0)));
        assert_eq!(fields(&p2, &s).shades, -1);
        // Another model shows its own tolerances (the entry keeps every model's fields).
        let mut f = fields(&p2, &s);
        f.range.model = 2;
        let (p3, _) = edited(&p2, &s, &f);
        assert_eq!(p3.color.model, ColorModel::Rgb);
        assert_eq!(fields(&p3, &s).range.a, 12);
    }

    #[test]
    fn loose_plus_is_the_fourth_shades_option() {
        let (mut p, s) = (green(), Settings::default());
        p.color.include = vec![ShadePreset::LoosePlus.entry(ColorModel::Oklch, Rgb::from_u32(0x86_04_A0))];
        assert_eq!(fields(&p, &s).shades, 3);
        let mut f = fields(&green(), &s);
        f.shades = 3;
        let (p2, _) = edited(&green(), &s, &f);
        assert_eq!(
            p2.color.include[0],
            ShadePreset::LoosePlus.entry(ColorModel::Oklch, Rgb::from_u32(0x3B_D1_6F))
        );
        assert_eq!(shade_preset(3), ShadePreset::LoosePlus);
    }

    #[test]
    fn the_hero_card_says_what_to_do() {
        let f = fields(&green(), &Settings::default());
        let idle = hero(&f, false, None);
        assert!(idle.enabled && !idle.danger);
        assert_eq!(idle.hint, "Switch to Sober and press F7 to start · F8 stops everything");
        let started = hero(&f, true, None);
        assert_eq!((started.button.as_str(), started.danger), ("■  Stop", true));
        assert_eq!(started.hint, "Watching the center of Sober · F8 stops everything");
        let none = hero(&fields(&Profile::new("x"), &Settings::default()), false, None);
        assert!(!none.enabled);
        assert_eq!(none.hint, "Pick a color to start");
        let mut hold = f.clone();
        hold.mode = 1;
        assert_eq!(
            hero(&hold, false, None).hint,
            "Switch to Sober and hold F7 to watch · F8 stops everything"
        );
        hold.window_name.clear();
        assert_eq!(hero(&hold, false, None).hint, "Hold F7 to watch · F8 stops everything");
        let refused = hero(&f, false, Some("Window mode comes in a later version."));
        assert_eq!(
            (refused.enabled, refused.hint.as_str()),
            (false, "Window mode comes in a later version.")
        );
    }
}
