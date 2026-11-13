// SPDX-License-Identifier: MIT
//! Profiles (PLAN §11, Appendix B, ADR 0008): what to watch, how to sample it, which colors
//! count, the trigger settings, the action and the guards.
//!
//! The engine is the only writer. Files are TOML, and the protocol's `set_profile` carries the
//! same structure as JSON. Every section and field has a default, so a hand-written file only
//! needs `schema_version` and `name`; unknown fields are errors, so a typo can't silently fall
//! back to a default.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::color::{ColorRules, CompiledRules};
use crate::pattern::{PatternError, PatternKind, SamplePattern};
use crate::schema::{self, LoadError, Loaded, Migration};
use crate::trigger::{MAX_CLICKS_PER_SEC_LIMIT, TriggerCfg};

/// The profile schema this build reads and writes.
pub const PROFILE_SCHEMA_VERSION: u32 = 1;

/// Upgrades from older profile schemas, oldest first (none yet).
const MIGRATIONS: &[Migration] = &[];

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    pub schema_version: u32,
    pub name: String,
    pub target: Target,
    pub sampling: Sampling,
    pub color: ColorRules,
    pub trigger: TriggerSettings,
    pub action: Action,
    pub guards: Guards,
}

impl Default for Profile {
    fn default() -> Self {
        Self::new("Default")
    }
}

impl Profile {
    /// A profile with every default and no color yet.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema_version: PROFILE_SCHEMA_VERSION,
            name: name.into(),
            target: Target::default(),
            sampling: Sampling::default(),
            color: ColorRules::default(),
            trigger: TriggerSettings::default(),
            action: Action::default(),
            guards: Guards::default(),
        }
    }

    /// Reads a profile file, migrating an older schema (§11). Validate the result before using it.
    pub fn load(text: &str) -> Result<Loaded<Self>, LoadError> {
        schema::load(text, PROFILE_SCHEMA_VERSION, MIGRATIONS)
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string(self)
    }

    /// The sample pattern. Validation guarantees it builds.
    pub fn pattern(&self) -> Result<SamplePattern, PatternError> {
        let s = &self.sampling;
        SamplePattern::from_profile(s.pattern, s.plus_distance, &s.offsets, s.k_enter, s.k_stay)
    }

    /// The color rules, ready for the lookup-table builder.
    pub fn rules(&self) -> CompiledRules {
        CompiledRules::new(&self.color)
    }

    /// The trigger's settings: the minimum gap is raised above the hold, and the check gap follows
    /// the speed (ADR 0007). Values are clamped to the trigger's hard limits even if validation
    /// was skipped.
    pub fn trigger_cfg(&self) -> TriggerCfg {
        let t = &self.trigger;
        let ms = |v: u32| Duration::from_millis(u64::from(v));
        TriggerCfg {
            confirm_samples: t.confirm_samples.max(1),
            confirm_min: ms(t.confirm_min_ms),
            rearm_absent: ms(t.rearm_absent_ms),
            rearm_absent_samples: t.rearm_absent_samples.max(1),
            min_fire_interval: TriggerCfg::min_gap(ms(t.min_fire_interval_ms), ms(self.action.hold_ms)),
            max_clicks_per_sec: t.max_clicks_per_sec.clamp(1, MAX_CLICKS_PER_SEC_LIMIT),
            fire_if_present_on_start: t.fire_if_present_on_start,
            max_check_gap: TriggerCfg::check_gap(self.sampling.speed.period()),
        }
    }

    /// Why Start would be refused although the profile is valid, in the UI's words (§12.7).
    pub fn start_blocker(&self) -> Option<&'static str> {
        if self.color.include.is_empty() {
            Some("Pick a color to start")
        } else if self.target.mode == TargetMode::Window && self.target.window.class.is_empty() {
            Some("Pick a window to start")
        } else {
            None
        }
    }
}

/// `[target]`: where to look.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Target {
    pub mode: TargetMode,
    /// Screen mode: a RandR monitor name, or `"primary"`.
    pub monitor: String,
    /// Window mode: the window to watch.
    pub window: WindowMatch,
    /// Pixels away from the exact center.
    pub offset: Offset,
}

impl Default for Target {
    fn default() -> Self {
        Self {
            mode: TargetMode::default(),
            monitor: "primary".to_owned(),
            window: WindowMatch::default(),
            offset: Offset::default(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetMode {
    /// The center of a monitor.
    #[default]
    Screen,
    /// The center of a window you pick.
    Window,
}

/// A window is matched by `WM_CLASS`, because window IDs change every time an app starts (§8).
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowMatch {
    pub class: String,
    pub title_contains: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Offset {
    pub x: i32,
    pub y: i32,
}

/// `[sampling]`: the pattern, how many of its pixels must agree, and how often to check.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Sampling {
    pub pattern: PatternKind,
    /// The plus pattern's arm length (1–31; default 2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plus_distance: Option<u16>,
    /// The custom pattern's `[dx, dy]` offsets.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub offsets: Vec<[i16; 2]>,
    /// Pixels that must be in the tight range to start (default: the §5.3 table).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub k_enter: Option<u8>,
    /// Pixels that must be in the wide range to stay present (default: ⌈k_enter / 2⌉).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub k_stay: Option<u8>,
    pub speed: Speed,
    pub backend: Backend,
}

/// Checks per second (§3): `"eco"`, `"balanced"`, `"max"`, or a number from 100 to 10,000.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Speed {
    /// 500 checks per second.
    Eco,
    /// 2,000 checks per second.
    #[default]
    Balanced,
    /// As fast as round trips allow, without sleeping.
    Max,
    PerSecond(u32),
}

impl Speed {
    pub const fn checks_per_second(self) -> Option<u32> {
        match self {
            Self::Eco => Some(500),
            Self::Balanced => Some(2000),
            Self::Max => None,
            Self::PerSecond(n) => Some(n),
        }
    }

    /// The time between checks; `None` for Max, which doesn't pace.
    pub fn period(self) -> Option<Duration> {
        self.checks_per_second()
            .map(|n| Duration::from_nanos(1_000_000_000 / u64::from(n.max(1))))
    }
}

impl Serialize for Speed {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Eco => serializer.serialize_str("eco"),
            Self::Balanced => serializer.serialize_str("balanced"),
            Self::Max => serializer.serialize_str("max"),
            Self::PerSecond(n) => serializer.serialize_u32(*n),
        }
    }
}

impl<'de> Deserialize<'de> for Speed {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = Speed;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("\"eco\", \"balanced\", \"max\" or a number of checks per second")
            }
            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Speed, E> {
                match s {
                    "eco" => Ok(Speed::Eco),
                    "balanced" => Ok(Speed::Balanced),
                    "max" => Ok(Speed::Max),
                    _ => Err(E::invalid_value(serde::de::Unexpected::Str(s), &self)),
                }
            }
            fn visit_u64<E: serde::de::Error>(self, n: u64) -> Result<Speed, E> {
                u32::try_from(n)
                    .map(Speed::PerSecond)
                    .map_err(|_| E::invalid_value(serde::de::Unexpected::Unsigned(n), &self))
            }
            fn visit_i64<E: serde::de::Error>(self, n: i64) -> Result<Speed, E> {
                u64::try_from(n)
                    .map_err(|_| E::invalid_value(serde::de::Unexpected::Signed(n), &self))
                    .and_then(|n| self.visit_u64(n))
            }
        }
        deserializer.deserialize_any(V)
    }
}

/// How the engine reads the screen (§5.2).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    #[default]
    Auto,
    Screen,
    ScreenShm,
    WindowBuffer,
    Damage,
}

/// `[trigger]` (§5.5).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TriggerSettings {
    pub confirm_samples: u32,
    pub confirm_min_ms: u32,
    /// *Gone after*.
    pub rearm_absent_ms: u32,
    pub rearm_absent_samples: u32,
    /// *Min gap*.
    pub min_fire_interval_ms: u32,
    /// *Safety stop*.
    pub max_clicks_per_sec: u32,
    pub fire_if_present_on_start: bool,
}

impl Default for TriggerSettings {
    fn default() -> Self {
        Self {
            confirm_samples: 1,
            confirm_min_ms: 0,
            rearm_absent_ms: 40,
            rearm_absent_samples: 2,
            min_fire_interval_ms: 25,
            max_clicks_per_sec: 15,
            fire_if_present_on_start: false,
        }
    }
}

/// `[action]`: what a click is (§6).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Action {
    pub kind: ActionKind,
    /// For `kind = "mouse"`.
    pub button: MouseButton,
    /// For `kind = "key"`: the keycode the key picker captured, and its name for display.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<KeySpec>,
    /// Pressed before the button or key and released after it.
    pub modifiers: Vec<Modifier>,
    pub hold_ms: u32,
    pub click_at: ClickAt,
}

impl Default for Action {
    fn default() -> Self {
        Self {
            kind: ActionKind::default(),
            button: MouseButton::default(),
            key: None,
            modifiers: Vec::new(),
            hold_ms: 10,
            click_at: ClickAt::default(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionKind {
    #[default]
    Mouse,
    Key,
}

/// Buttons 4–7 are scroll-wheel steps and aren't offered (§6).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    #[default]
    Left,
    Middle,
    Right,
    Back,
    Forward,
}

impl MouseButton {
    /// The X button number.
    pub const fn x_button(self) -> u8 {
        match self {
            Self::Left => 1,
            Self::Middle => 2,
            Self::Right => 3,
            Self::Back => 8,
            Self::Forward => 9,
        }
    }
}

/// A physical key: keycodes don't change with the keyboard layout (§6).
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeySpec {
    /// The X keycode, 8–255.
    pub keycode: u8,
    /// The key's name when it was picked, for display.
    pub name: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modifier {
    Shift,
    Ctrl,
    Alt,
    Super,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClickAt {
    /// A normal click wherever the pointer is.
    #[default]
    Pointer,
    /// Move the pointer to the watched spot first.
    WatchedSpot,
}

/// `[guards]` (§8).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Guards {
    /// *Only click in this window*.
    pub only_in_window: bool,
    pub pointer_over_window: bool,
    pub stop_when_locked: bool,
}

impl Default for Guards {
    fn default() -> Self {
        Self {
            only_in_window: true,
            pointer_over_window: false,
            stop_when_locked: true,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::color::{ColorEntry, ColorModel, Rgb, ShadePreset};

    /// The example profile in PLAN Appendix B, read from the plan itself so the two can't drift.
    pub(crate) fn appendix_b_profile() -> &'static str {
        let plan = include_str!("../../../docs/PLAN.md");
        let start = plan
            .find("# ~/.config/glint/profiles/sober-green-flash.toml")
            .expect("Appendix B profile");
        let len = plan[start..].find("```").expect("end of the code block");
        &plan[start..start + len]
    }

    #[test]
    fn appendix_b_parses_to_the_documented_values() {
        let loaded = Profile::load(appendix_b_profile()).unwrap();
        assert_eq!(loaded.migrated_from, None);
        let p = loaded.value;
        assert_eq!((p.schema_version, p.name.as_str()), (1, "Sober — green flash"));
        assert_eq!(p.target.mode, TargetMode::Window);
        assert_eq!(p.target.monitor, "primary");
        assert_eq!(
            p.target.window,
            WindowMatch {
                class: "sober".into(),
                title_contains: String::new()
            }
        );
        assert_eq!(p.target.offset, Offset { x: 0, y: 0 });
        assert_eq!(p.sampling.pattern, PatternKind::Square3);
        assert_eq!((p.sampling.k_enter, p.sampling.k_stay), (Some(5), Some(3)));
        assert_eq!((p.sampling.speed, p.sampling.backend), (Speed::Balanced, Backend::Auto));
        assert_eq!((p.color.model, p.color.exit_margin), (ColorModel::Oklch, 1.5));
        assert_eq!(
            p.color.include,
            vec![ShadePreset::Normal.entry(ColorModel::Oklch, Rgb::new(0x3B, 0xD1, 0x6F))]
        );
        assert_eq!(
            p.color.exclude,
            vec![ShadePreset::Tight.entry(ColorModel::Oklch, Rgb::new(0x6B, 0xD1, 0x3B))]
        );
        assert_eq!(p.trigger, TriggerSettings::default());
        assert_eq!(p.action, Action::default());
        assert_eq!(p.guards, Guards::default());
    }

    #[test]
    fn toml_and_json_round_trips() {
        let mut p = Profile::load(appendix_b_profile()).unwrap().value;
        p.sampling.speed = Speed::PerSecond(3000);
        p.sampling.pattern = PatternKind::Custom;
        p.sampling.offsets = vec![[0, 0], [-3, 4]];
        p.action = Action {
            kind: ActionKind::Key,
            key: Some(KeySpec {
                keycode: 26,
                name: "e".into(),
            }),
            modifiers: vec![Modifier::Shift, Modifier::Super],
            click_at: ClickAt::WatchedSpot,
            ..Action::default()
        };
        p.color.include.push(ColorEntry::new(Rgb::new(1, 2, 3)));
        let text = p.to_toml().unwrap();
        assert_eq!(Profile::load(&text).unwrap().value, p, "{text}");
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Profile>(&json).unwrap(), p, "{json}");
    }

    #[test]
    fn a_minimal_file_takes_every_default() {
        let p = Profile::load("schema_version = 1\nname = \"x\"").unwrap().value;
        assert_eq!(p, Profile::new("x"));
        assert_eq!(p.start_blocker(), Some("Pick a color to start"));
    }

    #[test]
    fn typos_and_bad_values_are_errors() {
        for bad in [
            "[target]\nmode = \"screen\"\nmontior = \"HDMI-1\"",
            "[sampling]\nspeed = \"fast\"",
            "[sampling]\nspeed = -5",
            "[sampling]\npattern = \"circle\"",
            "[trigger]\nrearm_ms = 40",
            "[action]\nbutton = \"wheel\"",
            "[action]\nkey = { keycode = 300, name = \"x\" }",
            "[action]\nmodifiers = [\"hyper\"]",
            "[guards]\nonly_in_window = \"yes\"",
            "[color]\nmodel = \"lab\"",
            "colour = 1",
        ] {
            let text = format!("schema_version = 1\nname = \"x\"\n{bad}");
            assert!(Profile::load(&text).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn speed_forms() {
        let speed = |v: &str| toml::from_str::<Sampling>(&format!("speed = {v}")).map(|s| s.speed);
        assert_eq!(speed("\"eco\"").unwrap(), Speed::Eco);
        assert_eq!(speed("\"max\"").unwrap(), Speed::Max);
        assert_eq!(speed("3000").unwrap(), Speed::PerSecond(3000));
        assert!(speed("4294967296").is_err());
        assert_eq!(Speed::Balanced.period(), Some(Duration::from_micros(500)));
        assert_eq!(Speed::Eco.period(), Some(Duration::from_millis(2)));
        assert_eq!(Speed::Max.period(), None);
        assert_eq!(serde_json::to_string(&Speed::PerSecond(250)).unwrap(), "250");
        assert_eq!(serde_json::from_str::<Speed>("\"balanced\"").unwrap(), Speed::Balanced);
    }

    #[test]
    fn trigger_cfg_applies_the_hold_and_the_speed() {
        let mut p = Profile::new("x");
        let c = p.trigger_cfg();
        assert_eq!(c, TriggerCfg::default());
        p.action.hold_ms = 200;
        p.sampling.speed = Speed::Eco;
        let c = p.trigger_cfg();
        assert_eq!(c.min_fire_interval, Duration::from_millis(201));
        assert_eq!(c.max_check_gap, Duration::from_millis(8));
        p.trigger.max_clicks_per_sec = 1000; // invalid, but the trigger's limit holds anyway
        p.trigger.rearm_absent_samples = 0;
        let c = p.trigger_cfg();
        assert_eq!((c.max_clicks_per_sec, c.rearm_absent_samples), (100, 1));
    }

    #[test]
    fn pattern_and_start_blockers() {
        let mut p = Profile::load(appendix_b_profile()).unwrap().value;
        let pattern = p.pattern().unwrap();
        assert_eq!((pattern.pixel_count(), pattern.k_enter(), pattern.k_stay()), (9, 5, 3));
        assert_eq!(p.start_blocker(), None);
        p.target.window.class.clear();
        assert_eq!(p.start_blocker(), Some("Pick a window to start"));
        p.target.mode = TargetMode::Screen;
        assert_eq!(p.start_blocker(), None);
        assert_eq!(MouseButton::Forward.x_button(), 9);
    }
}
