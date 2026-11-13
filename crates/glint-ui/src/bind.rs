// SPDX-License-Identifier: MIT
//! Between the engine's data and the window (PLAN §12): what the window shows for a profile, the
//! settings and the engine's state ([`fields`], [`hero`]), and the profile and settings its edits
//! make ([`edited`]). Plain data, tested without a window; [`apply`] copies it into the Slint
//! globals.

use glint_core::color::ShadePreset;
use glint_core::profile::{ActionKind, MouseButton, TargetMode};
use glint_core::settings::{ClosePolicy, HotkeyMode, Settings};
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
    /// Ask, keep running, quit.
    pub closing: i32,
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
        closing: index_of(&CLOSING, &settings.close_policy).max(0),
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
    p.guards.only_in_window = f.only_in_window;
    s.hotkeys.mode = if f.mode == 1 {
        HotkeyMode::Hold
    } else {
        HotkeyMode::Toggle
    };
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
    form.set_closing_index(f.closing);

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
        closing: form.get_closing_index(),
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
        let (p2, s2) = edited(&p, &s, &f);
        assert_eq!(fields(&p2, &s2), f);
        assert_eq!(
            p2.color.include[0],
            ShadePreset::Loose.entry(ColorModel::Oklch, Rgb::from_u32(0x3B_D1_6F))
        );
        assert_eq!(p2.action.button, MouseButton::Right);
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
