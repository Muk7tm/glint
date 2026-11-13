// SPDX-License-Identifier: GPL-3.0-or-later
//! Settings (PLAN §11, Appendix B, ADR 0008): the close policy, the active profile, hotkeys,
//! engine options and UI state. Like profiles, every field has a default and unknown fields are
//! errors.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::schema::{self, LoadError, Loaded, Migration};

/// The settings schema this build reads and writes.
pub const SETTINGS_SCHEMA_VERSION: u32 = 1;

/// Upgrades from older settings schemas, oldest first (none yet).
const MIGRATIONS: &[Migration] = &[];

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub schema_version: u32,
    pub active_profile: String,
    pub close_policy: ClosePolicy,
    /// Find the target window again when it reopens (never starts watching on its own).
    pub reacquire_window: bool,
    pub hotkeys: Hotkeys,
    pub engine: EngineSettings,
    pub ui: UiSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            active_profile: "Default".to_owned(),
            close_policy: ClosePolicy::default(),
            reacquire_window: true,
            hotkeys: Hotkeys::default(),
            engine: EngineSettings::default(),
            ui: UiSettings::default(),
        }
    }
}

impl Settings {
    /// Reads a settings file, migrating an older schema (§11). Validate the result before using it.
    pub fn load(text: &str) -> Result<Loaded<Self>, LoadError> {
        schema::load(text, SETTINGS_SCHEMA_VERSION, MIGRATIONS)
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string(self)
    }
}

/// What the window's close button does (§4.3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClosePolicy {
    #[default]
    Ask,
    KeepRunning,
    Quit,
}

/// `[hotkeys]` (§9). Each binding is written like `"F7"` or `"Ctrl+Shift+F9"`; an empty string
/// leaves start/stop or sample unbound. The emergency stop is always bound.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Hotkeys {
    pub start_stop: String,
    pub emergency_stop: String,
    pub sample_center: String,
    pub mode: HotkeyMode,
    pub delivery: Delivery,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            start_stop: "F7".to_owned(),
            emergency_stop: "F8".to_owned(),
            sample_center: "F6".to_owned(),
            mode: HotkeyMode::default(),
            delivery: Delivery::default(),
        }
    }
}

impl Hotkeys {
    /// The bindings with their field names, for validation and messages.
    pub fn bindings(&self) -> [(&'static str, &str); 3] {
        [
            ("start_stop", self.start_stop.as_str()),
            ("emergency_stop", self.emergency_stop.as_str()),
            ("sample_center", self.sample_center.as_str()),
        ]
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HotkeyMode {
    /// *Press to toggle*.
    #[default]
    Toggle,
    /// *Hold to start*: watch only while the key is held.
    Hold,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Delivery {
    /// XI2 raw events: the app also gets the key.
    #[default]
    Listen,
    /// `XGrabKey`: the app doesn't get the key.
    Capture,
}

/// A parsed hotkey: modifiers and one key, compared without regard to case.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
    /// An X key name such as `F7`, `Pause` or `KP_Add`.
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HotkeyError {
    #[error("{0:?} names no key; write it like F7 or Ctrl+Shift+F9")]
    NoKey(String),
    #[error("{0:?} isn't a key name; write it like F7 or Ctrl+Shift+F9")]
    BadKey(String),
    #[error("{0} is listed twice")]
    RepeatedModifier(&'static str),
}

impl Hotkey {
    /// Parses `"F7"`, `"Ctrl+Shift+F9"` and the like. An empty string means no hotkey.
    pub fn parse(s: &str) -> Result<Option<Self>, HotkeyError> {
        let s = s.trim();
        if s.is_empty() {
            return Ok(None);
        }
        let mut hotkey = Self {
            ctrl: false,
            alt: false,
            shift: false,
            super_key: false,
            key: String::new(),
        };
        let mut parts = s.split('+').map(str::trim).peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                if part.is_empty() {
                    return Err(HotkeyError::NoKey(s.to_owned()));
                }
                if !part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    return Err(HotkeyError::BadKey(part.to_owned()));
                }
                hotkey.key = part.to_owned();
                break;
            }
            let (name, flag) = match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => ("Ctrl", &mut hotkey.ctrl),
                "alt" => ("Alt", &mut hotkey.alt),
                "shift" => ("Shift", &mut hotkey.shift),
                "super" => ("Super", &mut hotkey.super_key),
                _ => return Err(HotkeyError::BadKey(part.to_owned())),
            };
            if *flag {
                return Err(HotkeyError::RepeatedModifier(name));
            }
            *flag = true;
        }
        Ok(Some(hotkey))
    }

    /// True if two hotkeys are the same key with the same modifiers.
    pub fn same_as(&self, other: &Self) -> bool {
        (self.ctrl, self.alt, self.shift, self.super_key) == (other.ctrl, other.alt, other.shift, other.super_key)
            && self.key.eq_ignore_ascii_case(&other.key)
    }

    /// Ctrl+Alt+F1…F12 switch virtual terminals and Ctrl+Alt+Backspace can kill the X server
    /// (§9), whatever other modifiers are held.
    pub fn is_blocked(&self) -> bool {
        if !(self.ctrl && self.alt) {
            return false;
        }
        let key = self.key.to_ascii_lowercase();
        let function_key = key
            .strip_prefix('f')
            .and_then(|n| n.parse::<u8>().ok())
            .is_some_and(|n| (1..=12).contains(&n));
        function_key || key == "backspace"
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
            (self.super_key, "Super"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&self.key)
    }
}

/// `[engine]`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EngineSettings {
    pub realtime: Realtime,
    pub cpu: CpuPin,
    pub log_level: LogLevel,
}

/// Real-time scheduling for the hot thread (§5.6).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Realtime {
    Off,
    #[default]
    Auto,
    Required,
}

/// CPU pinning for the hot thread (§5.6): `"none"`, `"auto"` or a core index.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum CpuPin {
    #[default]
    None,
    Auto,
    Core(u16),
}

impl Serialize for CpuPin {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::None => serializer.serialize_str("none"),
            Self::Auto => serializer.serialize_str("auto"),
            Self::Core(n) => serializer.serialize_u16(*n),
        }
    }
}

impl<'de> Deserialize<'de> for CpuPin {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = CpuPin;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("\"none\", \"auto\" or a CPU core index")
            }
            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<CpuPin, E> {
                match s {
                    "none" => Ok(CpuPin::None),
                    "auto" => Ok(CpuPin::Auto),
                    _ => Err(E::invalid_value(serde::de::Unexpected::Str(s), &self)),
                }
            }
            fn visit_u64<E: serde::de::Error>(self, n: u64) -> Result<CpuPin, E> {
                u16::try_from(n)
                    .map(CpuPin::Core)
                    .map_err(|_| E::invalid_value(serde::de::Unexpected::Unsigned(n), &self))
            }
            fn visit_i64<E: serde::de::Error>(self, n: i64) -> Result<CpuPin, E> {
                u64::try_from(n)
                    .map_err(|_| E::invalid_value(serde::de::Unexpected::Signed(n), &self))
                    .and_then(|n| self.visit_u64(n))
            }
        }
        deserializer.deserialize_any(V)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

/// `[ui]`: state the UI asks the engine to remember.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiSettings {
    pub more_options_open: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn appendix_b_settings() -> &'static str {
        let plan = include_str!("../../../docs/PLAN.md");
        let start = plan
            .find("# ~/.config/glint/settings.toml")
            .expect("Appendix B settings");
        let len = plan[start..].find("```").expect("end of the code block");
        &plan[start..start + len]
    }

    #[test]
    fn appendix_b_parses_to_the_defaults() {
        let s = Settings::load(appendix_b_settings()).unwrap().value;
        assert_eq!(
            s,
            Settings {
                active_profile: "Sober — green flash".into(),
                ..Settings::default()
            }
        );
        let again = Settings::load(&s.to_toml().unwrap()).unwrap().value;
        assert_eq!(again, s);
    }

    #[test]
    fn every_option_round_trips() {
        let s = Settings {
            close_policy: ClosePolicy::KeepRunning,
            reacquire_window: false,
            hotkeys: Hotkeys {
                start_stop: "Ctrl+F9".into(),
                sample_center: String::new(),
                mode: HotkeyMode::Hold,
                delivery: Delivery::Capture,
                ..Hotkeys::default()
            },
            engine: EngineSettings {
                realtime: Realtime::Required,
                cpu: CpuPin::Core(3),
                log_level: LogLevel::Debug,
            },
            ui: UiSettings {
                more_options_open: true,
            },
            ..Settings::default()
        };
        assert_eq!(Settings::load(&s.to_toml().unwrap()).unwrap().value, s);
        assert_eq!(
            serde_json::from_str::<Settings>(&serde_json::to_string(&s).unwrap()).unwrap(),
            s
        );
        for (text, cpu) in [
            ("\"none\"", CpuPin::None),
            ("\"auto\"", CpuPin::Auto),
            ("7", CpuPin::Core(7)),
        ] {
            assert_eq!(
                toml::from_str::<EngineSettings>(&format!("cpu = {text}")).unwrap().cpu,
                cpu
            );
        }
        for bad in [
            "cpu = -1",
            "cpu = 70000",
            "cpu = \"all\"",
            "realtime = \"on\"",
            "log_level = \"loud\"",
        ] {
            assert!(toml::from_str::<EngineSettings>(bad).is_err(), "{bad}");
        }
        assert!(Settings::load("schema_version = 1\n[hotkeys]\npanic = \"F8\"").is_err());
    }

    #[test]
    fn hotkey_grammar() {
        let hk = |s: &str| Hotkey::parse(s).unwrap().unwrap();
        assert_eq!(
            hk("F7"),
            Hotkey {
                ctrl: false,
                alt: false,
                shift: false,
                super_key: false,
                key: "F7".into()
            }
        );
        let h = hk(" control + shift+KP_Add ");
        assert!(h.ctrl && h.shift && !h.alt && !h.super_key && h.key == "KP_Add");
        assert_eq!(h.to_string(), "Ctrl+Shift+KP_Add");
        assert_eq!(Hotkey::parse("").unwrap(), None);
        assert_eq!(Hotkey::parse("Ctrl+"), Err(HotkeyError::NoKey("Ctrl+".into())));
        assert_eq!(Hotkey::parse("Hyper+F7"), Err(HotkeyError::BadKey("Hyper".into())));
        assert_eq!(Hotkey::parse("F7+F8"), Err(HotkeyError::BadKey("F7".into())));
        assert_eq!(Hotkey::parse("Num Lock"), Err(HotkeyError::BadKey("Num Lock".into())));
        assert_eq!(Hotkey::parse("Alt+alt+F4"), Err(HotkeyError::RepeatedModifier("Alt")));
        assert!(hk("f7").same_as(&hk("F7")));
        assert!(!hk("Shift+F7").same_as(&hk("F7")));
    }

    #[test]
    fn blocked_combinations() {
        let hk = |s: &str| Hotkey::parse(s).unwrap().unwrap();
        for blocked in ["Ctrl+Alt+F1", "Alt+Ctrl+f12", "Ctrl+Alt+Shift+F5", "Ctrl+Alt+BackSpace"] {
            assert!(hk(blocked).is_blocked(), "{blocked}");
        }
        for fine in [
            "Ctrl+F1",
            "Alt+F4",
            "Ctrl+Alt+F13",
            "Ctrl+Alt+F",
            "Ctrl+Alt+Delete",
            "F8",
        ] {
            assert!(!hk(fine).is_blocked(), "{fine}");
        }
    }
}
