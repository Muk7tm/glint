// SPDX-License-Identifier: MIT
//! Key names for the key pickers (§12.6). Slint reports named keys as private-use characters
//! (`slint::platform::Key`) and printable keys as the text they produce; Glint stores X keysym
//! names (`F7`, `space`, `a`), the ones `glint_x11::keymap::keysym_from_name` resolves. Printable
//! keys map to their US-layout key, so on other layouts the dropdown is the exact fallback.

use glint_x11::env::LocalDisplay;
use glint_x11::keymap::{Keymap, keysym_from_name};
use slint::platform::Key as SKey;

const NAMED: &[(SKey, &str)] = &[
    (SKey::Backspace, "BackSpace"),
    (SKey::Tab, "Tab"),
    (SKey::Return, "Return"),
    (SKey::Escape, "Escape"),
    (SKey::Delete, "Delete"),
    (SKey::Space, "space"),
    (SKey::UpArrow, "Up"),
    (SKey::DownArrow, "Down"),
    (SKey::LeftArrow, "Left"),
    (SKey::RightArrow, "Right"),
    (SKey::Insert, "Insert"),
    (SKey::Home, "Home"),
    (SKey::End, "End"),
    (SKey::PageUp, "Prior"),
    (SKey::PageDown, "Next"),
    (SKey::ScrollLock, "Scroll_Lock"),
    (SKey::Pause, "Pause"),
    (SKey::Menu, "Menu"),
    (SKey::F1, "F1"),
    (SKey::F2, "F2"),
    (SKey::F3, "F3"),
    (SKey::F4, "F4"),
    (SKey::F5, "F5"),
    (SKey::F6, "F6"),
    (SKey::F7, "F7"),
    (SKey::F8, "F8"),
    (SKey::F9, "F9"),
    (SKey::F10, "F10"),
    (SKey::F11, "F11"),
    (SKey::F12, "F12"),
    (SKey::F13, "F13"),
    (SKey::F14, "F14"),
    (SKey::F15, "F15"),
    (SKey::F16, "F16"),
];

fn printable(c: char) -> Option<&'static str> {
    Some(match c {
        '-' | '_' => "minus",
        '=' | '+' => "equal",
        '[' | '{' => "bracketleft",
        ']' | '}' => "bracketright",
        ';' | ':' => "semicolon",
        '\'' | '"' => "apostrophe",
        '`' | '~' => "grave",
        '\\' | '|' => "backslash",
        ',' | '<' => "comma",
        '.' | '>' => "period",
        '/' | '?' => "slash",
        '!' => "1",
        '@' => "2",
        '#' => "3",
        '$' => "4",
        '%' => "5",
        '^' => "6",
        '&' => "7",
        '*' => "8",
        '(' => "9",
        ')' => "0",
        _ => return None,
    })
}

/// The X key name for a Slint key event's text, if Glint can press or bind that key.
pub fn name_from_slint(text: &str) -> Option<String> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let name = match NAMED.iter().find(|(k, _)| char::from(*k) == c) {
        Some((_, n)) => (*n).to_owned(),
        None if c.is_ascii_alphanumeric() => c.to_ascii_lowercase().to_string(),
        None => printable(c)?.to_owned(),
    };
    keysym_from_name(&name).map(|_| name)
}

/// The keys the dropdowns offer: function keys first, then letters, digits and the rest.
pub fn names() -> Vec<String> {
    let mut out: Vec<String> = (1..=12).map(|n| format!("F{n}")).collect();
    out.extend(('a'..='z').chain('0'..='9').map(String::from));
    out.extend(
        [
            "space",
            "Return",
            "Tab",
            "BackSpace",
            "Up",
            "Down",
            "Left",
            "Right",
            "Insert",
            "Delete",
            "Home",
            "End",
        ]
        .map(String::from),
    );
    out
}

/// The start/stop key choices: the mouse side buttons first, as in Pulse, then the keys.
pub fn hotkey_names() -> Vec<String> {
    let mut out = vec!["Mouse4".to_owned(), "Mouse5".to_owned()];
    out.extend(names());
    out
}

/// A color typed as a hex code: `#3BD16F`, `3bd16f`, or the short `#3D7`.
pub fn parse_hex(text: &str) -> Option<glint_core::Rgb> {
    let hex = text.trim().trim_start_matches('#');
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let long: String = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect(),
        6 => hex.to_owned(),
        _ => return None,
    };
    u32::from_str_radix(&long, 16).ok().map(glint_core::Rgb::from_u32)
}

/// Keycodes for key names, from the X server's keymap (read once, on first use).
#[derive(Default)]
pub struct Resolver {
    keymap: Option<Keymap>,
}

impl Resolver {
    /// The keycode of `name` on this keyboard.
    pub fn keycode(&mut self, name: &str) -> Option<u8> {
        if self.keymap.is_none() {
            let display = LocalDisplay::from_env().ok()?;
            let conn = glint_x11::conn::open(display).ok()?;
            self.keymap = Keymap::fetch(&conn).ok();
        }
        self.keymap.as_ref()?.keycode(keysym_from_name(name)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_and_printable_keys() {
        let named = |k: SKey| name_from_slint(&char::from(k).to_string());
        assert_eq!(named(SKey::F7).as_deref(), Some("F7"));
        assert_eq!(named(SKey::Space).as_deref(), Some("space"));
        assert_eq!(named(SKey::PageUp).as_deref(), Some("Prior"));
        assert_eq!(name_from_slint("W").as_deref(), Some("w"));
        assert_eq!(name_from_slint("?").as_deref(), Some("slash"));
        assert_eq!(name_from_slint("é"), None);
        assert_eq!(name_from_slint("ab"), None);
        // Every name offered resolves.
        assert!(names().iter().all(|n| keysym_from_name(n).is_some()));
        assert!(NAMED.iter().all(|(_, n)| keysym_from_name(n).is_some()));
    }

    #[test]
    fn typed_colors() {
        use glint_core::Rgb;
        assert_eq!(parse_hex("#3BD16F"), Some(Rgb::from_u32(0x3B_D1_6F)));
        assert_eq!(parse_hex(" 3bd16f "), Some(Rgb::from_u32(0x3B_D1_6F)));
        assert_eq!(parse_hex("#3D7"), Some(Rgb::from_u32(0x33_DD_77)));
        for bad in ["", "#", "#12345", "#GGGGGG", "3bd16f0", "#+1234"] {
            assert_eq!(parse_hex(bad), None, "{bad}");
        }
        assert_eq!(&hotkey_names()[..3], ["Mouse4", "Mouse5", "F1"]);
    }
}
