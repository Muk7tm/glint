// SPDX-License-Identifier: MIT
//! Key names, keysyms and keycodes (PLAN §6, §9).
//!
//! A key action stores the keycode the key picker captured, which doesn't change with the
//! keyboard layout. Hotkeys and modifiers are names; they're resolved here against the server's
//! live keyboard mapping, and resolved again after a `MappingNotify`.

use x11rb::connection::Connection;
use x11rb::errors::{ConnectionError, ReplyError};
use x11rb::protocol::xproto::ConnectionExt as _;

/// Keysyms Glint presses or releases itself (values from X11's `keysymdef.h`).
pub mod keysym {
    pub const SHIFT_L: u32 = 0xffe1;
    pub const SHIFT_R: u32 = 0xffe2;
    pub const CONTROL_L: u32 = 0xffe3;
    pub const CONTROL_R: u32 = 0xffe4;
    pub const META_L: u32 = 0xffe7;
    pub const META_R: u32 = 0xffe8;
    pub const ALT_L: u32 = 0xffe9;
    pub const ALT_R: u32 = 0xffea;
    pub const SUPER_L: u32 = 0xffeb;
    pub const SUPER_R: u32 = 0xffec;
    pub const ISO_LEVEL3_SHIFT: u32 = 0xfe03;
}

/// Every modifier key that could be left pressed: what the defensive release lets go of (C10).
pub const MODIFIER_KEYSYMS: [u32; 11] = [
    keysym::SHIFT_L,
    keysym::SHIFT_R,
    keysym::CONTROL_L,
    keysym::CONTROL_R,
    keysym::META_L,
    keysym::META_R,
    keysym::ALT_L,
    keysym::ALT_R,
    keysym::SUPER_L,
    keysym::SUPER_R,
    keysym::ISO_LEVEL3_SHIFT,
];

/// The keysym for a key name, ignoring case: X's own names (`F7`, `Pause`, `KP_Add`,
/// `BackSpace`, `a`) and a few common spellings (`Esc`, `Enter`, `PageUp`).
pub fn keysym_from_name(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();
    let n = lower.as_str();
    let number = |s: &str| {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse::<u32>().ok())
            .flatten()
    };
    if let Some(f) = n.strip_prefix('f').and_then(number).filter(|f| (1..=35).contains(f)) {
        return Some(0xffbe + f - 1);
    }
    if let Some(d) = n.strip_prefix("kp_").and_then(number).filter(|d| *d <= 9) {
        return Some(0xffb0 + d);
    }
    if let [c] = n.as_bytes()
        && (c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return Some(u32::from(*c));
    }
    Some(match n {
        "escape" | "esc" => 0xff1b,
        "tab" => 0xff09,
        "return" | "enter" => 0xff0d,
        "backspace" => 0xff08,
        "space" => 0x0020,
        "insert" | "ins" => 0xff63,
        "delete" | "del" => 0xffff,
        "home" => 0xff50,
        "end" => 0xff57,
        "prior" | "page_up" | "pageup" | "pgup" => 0xff55,
        "next" | "page_down" | "pagedown" | "pgdn" => 0xff56,
        "left" => 0xff51,
        "up" => 0xff52,
        "right" => 0xff53,
        "down" => 0xff54,
        "pause" => 0xff13,
        "scroll_lock" => 0xff14,
        "print" => 0xff61,
        "menu" => 0xff67,
        "num_lock" => 0xff7f,
        "caps_lock" => 0xffe5,
        "kp_add" => 0xffab,
        "kp_subtract" => 0xffad,
        "kp_multiply" => 0xffaa,
        "kp_divide" => 0xffaf,
        "kp_decimal" => 0xffae,
        "kp_enter" => 0xff8d,
        "grave" => 0x0060,
        "minus" => 0x002d,
        "equal" => 0x003d,
        "bracketleft" => 0x005b,
        "bracketright" => 0x005d,
        "backslash" => 0x005c,
        "semicolon" => 0x003b,
        "apostrophe" => 0x0027,
        "comma" => 0x002c,
        "period" => 0x002e,
        "slash" => 0x002f,
        "shift_l" => keysym::SHIFT_L,
        "shift_r" => keysym::SHIFT_R,
        "control_l" => keysym::CONTROL_L,
        "control_r" => keysym::CONTROL_R,
        "alt_l" => keysym::ALT_L,
        "alt_r" => keysym::ALT_R,
        "super_l" => keysym::SUPER_L,
        "super_r" => keysym::SUPER_R,
        "meta_l" => keysym::META_L,
        "meta_r" => keysym::META_R,
        "iso_level3_shift" => keysym::ISO_LEVEL3_SHIFT,
        _ => return None,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum KeymapError {
    #[error(transparent)]
    Connection(#[from] ConnectionError),
    #[error(transparent)]
    Reply(#[from] ReplyError),
}

/// The server's keyboard mapping: which keysyms each keycode produces.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Keymap {
    min_keycode: u8,
    per_keycode: usize,
    keysyms: Vec<u32>,
}

impl Keymap {
    pub fn fetch(conn: &impl Connection) -> Result<Self, KeymapError> {
        let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
        let reply = conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
        Ok(Self::new(min, usize::from(reply.keysyms_per_keycode), reply.keysyms))
    }

    pub fn new(min_keycode: u8, keysyms_per_keycode: usize, keysyms: Vec<u32>) -> Self {
        Self {
            min_keycode,
            per_keycode: keysyms_per_keycode.max(1),
            keysyms,
        }
    }

    /// Every keycode that produces `keysym` on its own or with Shift (the first two columns).
    pub fn keycodes(&self, keysym: u32) -> impl Iterator<Item = u8> + '_ {
        let columns = self.per_keycode.min(2);
        self.keysyms
            .chunks(self.per_keycode)
            .enumerate()
            .filter_map(move |(i, syms)| {
                let code = usize::from(self.min_keycode) + i;
                (syms[..columns.min(syms.len())].contains(&keysym) && code <= 255).then_some(code as u8)
            })
    }

    /// The first keycode that produces `keysym`.
    pub fn keycode(&self, keysym: u32) -> Option<u8> {
        self.keycodes(keysym).next()
    }

    /// Every keycode of a modifier key, for the defensive release (C10).
    pub fn modifier_keycodes(&self) -> Vec<u8> {
        let mut out: Vec<u8> = MODIFIER_KEYSYMS.iter().flat_map(|&s| self.keycodes(s)).collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// Every X name in the table above, so the test can check them against `keysymdef.h`.
    const X_NAMES: &[&str] = &[
        "Escape",
        "Tab",
        "Return",
        "BackSpace",
        "space",
        "Insert",
        "Delete",
        "Home",
        "End",
        "Prior",
        "Page_Up",
        "Next",
        "Page_Down",
        "Left",
        "Up",
        "Right",
        "Down",
        "Pause",
        "Scroll_Lock",
        "Print",
        "Menu",
        "Num_Lock",
        "Caps_Lock",
        "KP_Add",
        "KP_Subtract",
        "KP_Multiply",
        "KP_Divide",
        "KP_Decimal",
        "KP_Enter",
        "grave",
        "minus",
        "equal",
        "bracketleft",
        "bracketright",
        "backslash",
        "semicolon",
        "apostrophe",
        "comma",
        "period",
        "slash",
        "Shift_L",
        "Shift_R",
        "Control_L",
        "Control_R",
        "Alt_L",
        "Alt_R",
        "Super_L",
        "Super_R",
        "Meta_L",
        "Meta_R",
        "ISO_Level3_Shift",
    ];

    #[test]
    fn names_match_keysymdef() {
        let Ok(header) = std::fs::read_to_string("/usr/include/X11/keysymdef.h") else {
            eprintln!("skipped: /usr/include/X11/keysymdef.h not found");
            return;
        };
        let defs: HashMap<&str, u32> = header
            .lines()
            .filter_map(|l| {
                let mut it = l.strip_prefix("#define XK_")?.split_whitespace();
                let (name, value) = (it.next()?, it.next()?);
                Some((name, u32::from_str_radix(value.strip_prefix("0x")?, 16).ok()?))
            })
            .collect();
        let mut names: Vec<String> = X_NAMES.iter().map(|s| (*s).to_owned()).collect();
        names.extend((1..=35).map(|f| format!("F{f}")));
        names.extend((0..=9).map(|d| format!("KP_{d}")));
        names.extend(('a'..='z').chain('0'..='9').map(String::from));
        for name in &names {
            assert_eq!(keysym_from_name(name), defs.get(name.as_str()).copied(), "{name}");
        }
    }

    #[test]
    fn spellings_and_unknown_names() {
        assert_eq!(keysym_from_name("f7"), Some(0xffc4));
        assert_eq!(keysym_from_name("ESC"), keysym_from_name("Escape"));
        assert_eq!(keysym_from_name("Enter"), keysym_from_name("Return"));
        assert_eq!(keysym_from_name("PageUp"), keysym_from_name("Prior"));
        assert_eq!(keysym_from_name("E"), Some(u32::from(b'e')));
        for unknown in ["", "F0", "F36", "F7a", "f+7", "KP_10", "Hyper_L", "ä"] {
            assert_eq!(keysym_from_name(unknown), None, "{unknown}");
        }
    }

    #[test]
    fn keycodes_from_a_mapping() {
        // keycode 8: (a, A); 9: (F7, F7); 10: (Shift_L, none); 11: (b, B, a) — a third column
        // doesn't count.
        let map = Keymap::new(
            8,
            3,
            vec![
                0x61,
                0x41,
                0,
                0xffc4,
                0xffc4,
                0,
                keysym::SHIFT_L,
                0,
                0,
                0x62,
                0x42,
                0x61,
            ],
        );
        assert_eq!(map.keycodes(0x61).collect::<Vec<_>>(), [8]);
        assert_eq!(map.keycode(0x41), Some(8)); // shifted column
        assert_eq!(map.keycode(0xffc4), Some(9));
        assert_eq!(map.keycode(0x63), None);
        assert_eq!(map.modifier_keycodes(), [10]);
    }
}
