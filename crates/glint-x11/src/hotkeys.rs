// SPDX-License-Identifier: GPL-3.0-or-later
//! Hotkeys (PLAN §9): XInput 2 raw key events on the root window. Since XI 2.1 these arrive even
//! while another program holds a keyboard grab, so F8 works over a fullscreen game. Raw events
//! don't consume the key: the focused app gets it too.
//!
//! Events whose source is one of the "Virtual core XTEST" devices are ignored, so Glint's own key
//! presses can never trigger a hotkey. Modifiers are tracked from the same raw events. Mouse side
//! buttons (`Mouse4` and `Mouse5`, X buttons 8 and 9) work as hotkeys too, from raw button events.

use glint_core::settings::{Hotkey, Hotkeys};
use x11rb::connection::Connection;
use x11rb::errors::{ConnectionError, ReplyError};
use x11rb::protocol::Event;
use x11rb::protocol::xinput::{self, ConnectionExt as _, KeyEventFlags, XIEventMask};

use crate::keymap::{Keymap, keysym, keysym_from_name};

/// `XIAllMasterDevices`: one raw event per key, with the physical source in `sourceid`.
const ALL_MASTER_DEVICES: u16 = 1;
/// `XIAllDevices`, for the device query.
const ALL_DEVICES: u16 = 0;

#[derive(Debug, thiserror::Error)]
pub enum HotkeyError {
    #[error(transparent)]
    Connection(#[from] ConnectionError),
    #[error(transparent)]
    Reply(#[from] ReplyError),
}

/// Asks for raw key and button presses and releases on `root`. Connection C must have announced
/// XI 2.2 (the capability probe does).
pub fn select_raw_keys(conn: &impl Connection, root: u32) -> Result<(), HotkeyError> {
    let mask = xinput::EventMask {
        deviceid: ALL_MASTER_DEVICES,
        mask: vec![
            XIEventMask::RAW_KEY_PRESS
                | XIEventMask::RAW_KEY_RELEASE
                | XIEventMask::RAW_BUTTON_PRESS
                | XIEventMask::RAW_BUTTON_RELEASE,
        ],
    };
    conn.xinput_xi_select_events(root, &[mask])?.check()?;
    Ok(())
}

/// The ids of the XTEST devices: input that XTest made, Glint's own included.
pub fn xtest_devices(conn: &impl Connection) -> Result<Vec<u16>, HotkeyError> {
    let reply = conn.xinput_xi_query_device(ALL_DEVICES)?.reply()?;
    Ok(reply
        .infos
        .iter()
        .filter(|d| String::from_utf8_lossy(&d.name).contains("XTEST"))
        .map(|d| d.deviceid)
        .collect())
}

/// One raw key or button event.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RawKey {
    /// The keycode, or the button number when `button` is set.
    pub keycode: u8,
    /// A mouse button, not a key.
    pub button: bool,
    pub press: bool,
    /// The physical (slave) device it came from.
    pub source: u16,
    /// An auto-repeat, not a new press.
    pub repeat: bool,
}

/// The raw key or button event in `event`, if it is one.
pub fn raw_key(event: &Event) -> Option<RawKey> {
    if let Event::XinputRawButtonPress(e) | Event::XinputRawButtonRelease(e) = event {
        return Some(RawKey {
            keycode: u8::try_from(e.detail).ok()?,
            button: true,
            press: matches!(event, Event::XinputRawButtonPress(_)),
            source: e.sourceid,
            repeat: false,
        });
    }
    let (e, press) = match event {
        Event::XinputRawKeyPress(e) => (e, true),
        Event::XinputRawKeyRelease(e) => (e, false),
        _ => return None,
    };
    Some(RawKey {
        keycode: u8::try_from(e.detail).ok()?,
        button: false,
        press,
        source: e.sourceid,
        repeat: e.flags & KeyEventFlags::KEY_REPEAT != KeyEventFlags::from(0u32),
    })
}

/// What a hotkey does (§9).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HotkeyAction {
    StartStop,
    EmergencyStop,
    SampleCenter,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
struct Mods {
    ctrl: bool,
    alt: bool,
    shift: bool,
    super_key: bool,
}

/// A binding resolved to keycodes on the current keymap, or to a mouse side button.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Binding {
    pub action: HotkeyAction,
    keycodes: Vec<u8>,
    button: Option<u8>,
    mods: Mods,
}

/// The X button of a side-button hotkey name: `Mouse4` is button 8 (back), `Mouse5` button 9
/// (forward), the names Pulse uses.
pub fn side_button(name: &str) -> Option<u8> {
    match name.to_ascii_lowercase().as_str() {
        "mouse4" => Some(8),
        "mouse5" => Some(9),
        _ => None,
    }
}

/// The hotkeys of `settings` on `keymap`. Bindings whose key the keymap doesn't have are reported
/// and left out; empty bindings are unbound.
pub fn bindings(settings: &Hotkeys, keymap: &Keymap) -> (Vec<Binding>, Vec<String>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    for (action, text) in [
        (HotkeyAction::StartStop, &settings.start_stop),
        (HotkeyAction::EmergencyStop, &settings.emergency_stop),
        (HotkeyAction::SampleCenter, &settings.sample_center),
    ] {
        let hotkey = match Hotkey::parse(text) {
            Ok(Some(h)) => h,
            Ok(None) => continue,
            Err(e) => {
                problems.push(e.to_string());
                continue;
            }
        };
        let button = side_button(&hotkey.key);
        let keycodes: Vec<u8> = keysym_from_name(&hotkey.key)
            .map(|s| keymap.keycodes(s).collect())
            .unwrap_or_default();
        if keycodes.is_empty() && button.is_none() {
            problems.push(format!("{hotkey}: this keyboard has no key named {:?}", hotkey.key));
            continue;
        }
        let mods = Mods {
            ctrl: hotkey.ctrl,
            alt: hotkey.alt,
            shift: hotkey.shift,
            super_key: hotkey.super_key,
        };
        out.push(Binding {
            action,
            keycodes,
            button,
            mods,
        });
    }
    (out, problems)
}

/// Which modifier a keycode is, on `keymap`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ModifierKeys {
    ctrl: Vec<u8>,
    alt: Vec<u8>,
    shift: Vec<u8>,
    super_key: Vec<u8>,
}

impl ModifierKeys {
    pub fn new(keymap: &Keymap) -> Self {
        let codes = |syms: &[u32]| syms.iter().flat_map(|&s| keymap.keycodes(s)).collect();
        Self {
            ctrl: codes(&[keysym::CONTROL_L, keysym::CONTROL_R]),
            alt: codes(&[keysym::ALT_L, keysym::ALT_R, keysym::META_L, keysym::META_R]),
            shift: codes(&[keysym::SHIFT_L, keysym::SHIFT_R]),
            super_key: codes(&[keysym::SUPER_L, keysym::SUPER_R]),
        }
    }

    fn is_modifier(&self, keycode: u8) -> bool {
        [&self.ctrl, &self.alt, &self.shift, &self.super_key]
            .iter()
            .any(|v| v.contains(&keycode))
    }
}

/// Turns raw key events into hotkey presses and releases.
#[derive(Clone, Debug, Default)]
pub struct Matcher {
    xtest: Vec<u16>,
    modifier_keys: ModifierKeys,
    /// Modifier keycodes held right now (by real keyboards).
    held: Vec<u8>,
    /// The binding whose key or button is down, so its release matches even if modifiers
    /// changed.
    down: Option<(u8, bool, HotkeyAction)>,
}

impl Matcher {
    pub fn new(xtest_devices: Vec<u16>, modifier_keys: ModifierKeys) -> Self {
        Self {
            xtest: xtest_devices,
            modifier_keys,
            held: Vec::new(),
            down: None,
        }
    }

    /// Follows a keymap change.
    pub fn set_modifier_keys(&mut self, modifier_keys: ModifierKeys) {
        self.modifier_keys = modifier_keys;
        self.held.clear();
    }

    fn mods(&self) -> Mods {
        let any = |codes: &[u8]| self.held.iter().any(|k| codes.contains(k));
        let m = &self.modifier_keys;
        Mods {
            ctrl: any(&m.ctrl),
            alt: any(&m.alt),
            shift: any(&m.shift),
            super_key: any(&m.super_key),
        }
    }

    /// The hotkey that `key` presses (`true`) or releases (`false`), if any.
    pub fn feed(&mut self, key: RawKey, bindings: &[Binding]) -> Option<(HotkeyAction, bool)> {
        if self.xtest.contains(&key.source) || key.repeat {
            return None;
        }
        if !key.button && self.modifier_keys.is_modifier(key.keycode) {
            self.held.retain(|&k| k != key.keycode);
            if key.press {
                self.held.push(key.keycode);
            }
            return None;
        }
        if !key.press {
            return match self.down {
                Some((code, button, action)) if code == key.keycode && button == key.button => {
                    self.down = None;
                    Some((action, false))
                }
                _ => None,
            };
        }
        let mods = self.mods();
        let b = bindings.iter().find(|b| {
            b.mods == mods
                && if key.button {
                    b.button == Some(key.keycode)
                } else {
                    b.keycodes.contains(&key.keycode)
                }
        })?;
        self.down = Some((key.keycode, key.button, b.action));
        Some((b.action, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// keycode 8 Shift_L, 9 Control_L, 10 F7, 11 F8, 12 F6, 13 'e'
    fn keymap() -> Keymap {
        Keymap::new(
            8,
            1,
            vec![keysym::SHIFT_L, keysym::CONTROL_L, 0xffc4, 0xffc5, 0xffc3, 0x65],
        )
    }

    const KEYBOARD: u16 = 9;
    const XTEST: u16 = 4;

    fn key(keycode: u8, press: bool) -> RawKey {
        RawKey {
            keycode,
            button: false,
            press,
            source: KEYBOARD,
            repeat: false,
        }
    }

    fn setup(start_stop: &str) -> (Matcher, Vec<Binding>) {
        let map = keymap();
        let hotkeys = Hotkeys {
            start_stop: start_stop.into(),
            ..Hotkeys::default()
        };
        let (b, problems) = bindings(&hotkeys, &map);
        assert!(problems.is_empty(), "{problems:?}");
        (Matcher::new(vec![XTEST], ModifierKeys::new(&map)), b)
    }

    #[test]
    fn the_default_keys() {
        let (mut m, b) = setup("F7");
        assert_eq!(m.feed(key(10, true), &b), Some((HotkeyAction::StartStop, true)));
        assert_eq!(m.feed(key(10, false), &b), Some((HotkeyAction::StartStop, false)));
        assert_eq!(m.feed(key(11, true), &b), Some((HotkeyAction::EmergencyStop, true)));
        assert_eq!(m.feed(key(12, true), &b), Some((HotkeyAction::SampleCenter, true)));
        assert_eq!(m.feed(key(13, true), &b), None);
    }

    #[test]
    fn glints_own_input_and_repeats_never_match() {
        let (mut m, b) = setup("F7");
        assert_eq!(
            m.feed(
                RawKey {
                    source: XTEST,
                    ..key(10, true)
                },
                &b
            ),
            None
        );
        assert_eq!(
            m.feed(
                RawKey {
                    repeat: true,
                    ..key(11, true)
                },
                &b
            ),
            None
        );
        // An XTEST shift doesn't count as a held modifier either.
        let (mut m, b) = setup("Shift+F7");
        m.feed(
            RawKey {
                source: XTEST,
                ..key(8, true)
            },
            &b,
        );
        assert_eq!(m.feed(key(10, true), &b), None);
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let (mut m, b) = setup("Ctrl+F7");
        assert_eq!(m.feed(key(10, true), &b), None);
        m.feed(key(9, true), &b); // ctrl down
        assert_eq!(m.feed(key(10, true), &b), Some((HotkeyAction::StartStop, true)));
        m.feed(key(9, false), &b); // ctrl up before F7 up: the release still matches
        assert_eq!(m.feed(key(10, false), &b), Some((HotkeyAction::StartStop, false)));
        m.feed(key(8, true), &b);
        m.feed(key(9, true), &b); // ctrl+shift is not ctrl
        assert_eq!(m.feed(key(10, true), &b), None);
        // F8 is bound without modifiers, so it doesn't fire with ctrl+shift held.
        assert_eq!(m.feed(key(11, true), &b), None);
    }

    #[test]
    fn mouse_side_buttons() {
        let (mut m, b) = setup("Mouse4");
        let button = |n: u8, press: bool| RawKey {
            keycode: n,
            button: true,
            press,
            source: KEYBOARD,
            repeat: false,
        };
        assert_eq!(m.feed(button(8, true), &b), Some((HotkeyAction::StartStop, true)));
        assert_eq!(m.feed(button(8, false), &b), Some((HotkeyAction::StartStop, false)));
        // The forward button, a left click, and key 8 (Shift_L here) are something else.
        assert_eq!(m.feed(button(9, true), &b), None);
        assert_eq!(m.feed(button(1, true), &b), None);
        assert_eq!(m.feed(key(8, true), &b), None);
        // Glint's own Back clicks never count.
        let own = RawKey {
            source: XTEST,
            ..button(8, true)
        };
        assert_eq!(m.feed(own, &b), None);
        assert_eq!(side_button("mouse5"), Some(9));
    }

    #[test]
    fn unbound_and_unknown_keys() {
        let map = keymap();
        let hotkeys = Hotkeys {
            start_stop: String::new(),
            sample_center: "F13".into(),
            ..Hotkeys::default()
        };
        let (b, problems) = bindings(&hotkeys, &map);
        assert_eq!(
            b.iter().map(|b| b.action).collect::<Vec<_>>(),
            [HotkeyAction::EmergencyStop]
        );
        assert_eq!(problems, ["F13: this keyboard has no key named \"F13\""]);
    }
}
