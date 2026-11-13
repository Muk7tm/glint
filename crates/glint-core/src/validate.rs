// SPDX-License-Identifier: MIT
//! Validation (PLAN §11, ADR 0008). The engine runs these before applying a profile or settings.
//! An error rejects the change and names the field, so the UI can show the message next to it;
//! a warning is shown but the change is applied.

use serde::{Deserialize, Serialize};

use crate::color::rule::is_achromatic;
use crate::color::{ColorEntry, LutStats};
use crate::pattern::{MAX_REACH, PatternError, PatternKind, Placement, Rect, SamplePattern};
use crate::profile::{Action, ActionKind, MouseButton, PROFILE_SCHEMA_VERSION, Profile, Speed};
use crate::settings::{Hotkey, Hotkeys, SETTINGS_SCHEMA_VERSION, Settings};

/// Longest profile name.
pub const MAX_NAME_CHARS: usize = 64;
/// Most include colors in one profile.
pub const MAX_INCLUDE: usize = 16;
/// Most exclude colors in one profile.
pub const MAX_EXCLUDE: usize = 32;
/// Largest offset from the center on either axis; the real limit is the monitor or window.
pub const MAX_OFFSET: i32 = 16_384;

/// One problem, with the field it belongs to, written like `sampling.k_enter` or
/// `color.include[0].tol_h`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Issue {
    pub field: String,
    pub message: String,
}

impl Issue {
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Report {
    /// Any error rejects the change.
    pub errors: Vec<Issue>,
    /// Shown, but the change is applied.
    pub warnings: Vec<Issue>,
}

impl Report {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    fn error(&mut self, field: impl Into<String>, message: impl Into<String>) {
        self.errors.push(Issue::new(field, message));
    }

    fn warn(&mut self, field: impl Into<String>, message: impl Into<String>) {
        self.warnings.push(Issue::new(field, message));
    }

    /// Checks `lo ≤ value ≤ hi` for a whole number.
    fn range(&mut self, field: &str, value: u32, lo: u32, hi: u32) {
        if !(lo..=hi).contains(&value) {
            self.error(field, format!("Must be between {lo} and {hi}."));
        }
    }

    /// Checks `0 < value ≤ hi` for a tolerance.
    fn tolerance(&mut self, field: String, value: Option<f64>, hi: f64) {
        match value {
            Some(v) if !v.is_finite() => self.error(field, "Must be a number."),
            Some(v) if v <= 0.0 || v > hi => self.error(field, format!("Must be more than 0 and at most {hi}.")),
            _ => {}
        }
    }
}

/// Every rule a profile must follow on its own. Rules that need the screen (the offset against
/// the monitor or window) are in [`placement`]; rules that need the settings (the action key
/// against the hotkeys) are in [`action_against_hotkeys`].
pub fn profile(p: &Profile) -> Report {
    let mut r = Report::default();
    if p.schema_version != PROFILE_SCHEMA_VERSION {
        r.error("schema_version", format!("Must be {PROFILE_SCHEMA_VERSION}."));
    }
    name(&mut r, &p.name);

    if p.target.monitor.trim().is_empty() {
        r.error("target.monitor", "Name a monitor, or use \"primary\".");
    }
    for (field, v) in [
        ("target.offset.x", p.target.offset.x),
        ("target.offset.y", p.target.offset.y),
    ] {
        if v.abs() > MAX_OFFSET {
            r.error(field, format!("Must be between -{MAX_OFFSET} and {MAX_OFFSET}."));
        }
    }

    sampling(&mut r, p);

    let c = &p.color;
    if !(1.0..=3.0).contains(&c.exit_margin) {
        r.error("color.exit_margin", "Must be between 1.0 and 3.0.");
    }
    if c.include.len() > MAX_INCLUDE {
        r.error("color.include", format!("At most {MAX_INCLUDE} colors."));
    }
    if c.exclude.len() > MAX_EXCLUDE {
        r.error("color.exclude", format!("At most {MAX_EXCLUDE} colors."));
    }
    for (list, entries) in [("include", &c.include), ("exclude", &c.exclude)] {
        for (i, e) in entries.iter().enumerate() {
            tolerances(&mut r, &format!("color.{list}[{i}]"), e);
        }
    }
    for (i, e) in c.include.iter().enumerate() {
        if is_achromatic(c.model, e.hex) {
            r.warn(
                format!("color.include[{i}].hex"),
                "Grays are hard to tell apart from backgrounds.",
            );
        }
    }

    let t = &p.trigger;
    r.range("trigger.confirm_samples", t.confirm_samples, 1, 10);
    r.range("trigger.confirm_min_ms", t.confirm_min_ms, 0, 50);
    r.range("trigger.rearm_absent_ms", t.rearm_absent_ms, 0, 2000);
    if t.rearm_absent_ms < 15 {
        r.warn(
            "trigger.rearm_absent_ms",
            "Under 15 ms, flicker can count as a new appearance.",
        );
    }
    r.range("trigger.rearm_absent_samples", t.rearm_absent_samples, 1, 100);
    r.range("trigger.min_fire_interval_ms", t.min_fire_interval_ms, 0, 1000);
    r.range("trigger.max_clicks_per_sec", t.max_clicks_per_sec, 1, 100);

    action(&mut r, &p.action);
    if p.action.hold_ms >= t.min_fire_interval_ms && p.action.hold_ms <= 200 {
        r.warn(
            "trigger.min_fire_interval_ms",
            format!(
                "Clicks stay at least {} ms apart, because each is held {} ms.",
                p.action.hold_ms + 1,
                p.action.hold_ms
            ),
        );
    }
    r
}

fn name(r: &mut Report, name: &str) {
    if name.trim().is_empty() {
        r.error("name", "Give the profile a name.");
    } else if name.chars().count() > MAX_NAME_CHARS {
        r.error("name", format!("At most {MAX_NAME_CHARS} characters."));
    } else if name.chars().any(char::is_control) {
        r.error("name", "No control characters.");
    }
}

fn sampling(r: &mut Report, p: &Profile) {
    let s = &p.sampling;
    if let Some(d) = s.plus_distance
        && !(1..=MAX_REACH).contains(&d)
    {
        r.error("sampling.plus_distance", format!("Must be between 1 and {MAX_REACH}."));
        return;
    }
    if let Err(e) = p.pattern() {
        let field = match e {
            PatternError::KEnter { .. } => "sampling.k_enter",
            PatternError::KStay { .. } => "sampling.k_stay",
            _ if s.pattern == PatternKind::Custom => "sampling.offsets",
            _ => "sampling.pattern",
        };
        r.error(field, sentence(&e.to_string()));
    }
    match s.speed {
        Speed::PerSecond(n) if !(100..=10_000).contains(&n) => {
            r.error(
                "sampling.speed",
                "Must be eco, balanced, max, or 100 to 10,000 checks per second.",
            );
        }
        Speed::Max => r.warn("sampling.speed", "Max keeps one CPU core busy."),
        _ => {}
    }
}

fn tolerances(r: &mut Report, at: &str, e: &ColorEntry) {
    // Every stored tolerance must be legal, including those another color model would use.
    r.tolerance(format!("{at}.tol_l"), e.tol_l, 1.0);
    r.tolerance(format!("{at}.tol_c"), e.tol_c, 0.5);
    r.tolerance(format!("{at}.tol_h"), e.tol_h, 180.0);
    r.tolerance(format!("{at}.tol_s"), e.tol_s, 1.0);
    r.tolerance(format!("{at}.tol_v"), e.tol_v, 1.0);
    // tol_rgb is a whole number from 0 (exact match) to 255, which its type already guarantees.
}

fn action(r: &mut Report, a: &Action) {
    if a.kind == ActionKind::Key {
        match &a.key {
            None => r.error("action.key", "Pick a key."),
            Some(k) => {
                if k.keycode < 8 {
                    r.error("action.key.keycode", "Must be between 8 and 255.");
                }
                if k.name.trim().is_empty() {
                    r.error("action.key.name", "Give the key a name.");
                }
            }
        }
    }
    r.range("action.hold_ms", a.hold_ms, 0, 200);
    for (i, m) in a.modifiers.iter().enumerate() {
        if a.modifiers[..i].contains(m) {
            r.error("action.modifiers", "Each modifier at most once.");
            break;
        }
    }
}

/// Upper-cases the first letter and ends with a period, for the UI.
fn sentence(s: &str) -> String {
    let mut chars = s.chars();
    let first = chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_default();
    format!("{first}{}.", chars.as_str())
}

/// Every rule settings must follow on their own.
pub fn settings(s: &Settings) -> Report {
    let mut r = Report::default();
    if s.schema_version != SETTINGS_SCHEMA_VERSION {
        r.error("schema_version", format!("Must be {SETTINGS_SCHEMA_VERSION}."));
    }
    if s.active_profile.trim().is_empty() {
        r.error("active_profile", "Name the active profile.");
    }
    let mut parsed: Vec<(&str, Hotkey)> = Vec::new();
    for (field, text) in s.hotkeys.bindings() {
        let field_path = format!("hotkeys.{field}");
        match Hotkey::parse(text) {
            Err(e) => r.error(field_path, sentence(&e.to_string())),
            Ok(None) if field == "emergency_stop" => r.error(field_path, "The emergency stop needs a key."),
            Ok(None) => {}
            Ok(Some(h)) if h.is_blocked() => r.error(field_path, format!("{h} is reserved by the system.")),
            Ok(Some(h)) => {
                if let Some((other, _)) = parsed.iter().find(|(_, p)| p.same_as(&h)) {
                    r.error(field_path, format!("{h} is already the {} key.", describe(other)));
                }
                parsed.push((field, h));
            }
        }
    }
    r
}

fn describe(field: &str) -> &'static str {
    match field {
        "start_stop" => "start/stop",
        "emergency_stop" => "emergency stop",
        _ => "sample",
    }
}

/// The action key must not also be a hotkey (§11, `action-equals-hotkey`). Keys are compared by
/// name, ignoring modifiers: the stricter reading.
pub fn action_against_hotkeys(a: &Action, hotkeys: &Hotkeys) -> Report {
    let mut r = Report::default();
    // The side buttons are hotkeys by the names Mouse4 and Mouse5.
    let name = match (a.kind, &a.key) {
        (ActionKind::Key, Some(key)) => key.name.trim().to_owned(),
        (ActionKind::Mouse, _) if a.button == MouseButton::Back => "Mouse4".to_owned(),
        (ActionKind::Mouse, _) if a.button == MouseButton::Forward => "Mouse5".to_owned(),
        _ => return r,
    };
    for (field, text) in hotkeys.bindings() {
        if let Ok(Some(h)) = Hotkey::parse(text)
            && h.key.eq_ignore_ascii_case(&name)
        {
            r.error(
                "action.key",
                format!("{} is also the {} hotkey.", h.key, describe(field)),
            );
        }
    }
    r
}

/// Where the pattern goes at Start: the offset must keep the whole pattern on the monitor or
/// inside the window (§11). Returns the capture rectangle.
pub fn placement(pattern: &SamplePattern, area: Rect, offset: (i32, i32)) -> Result<Rect, Issue> {
    match pattern.place(area, offset) {
        Ok(Placement { rect, clamped: false }) => Ok(rect),
        Ok(Placement { clamped: true, .. }) => Err(Issue::new(
            "target.offset",
            "The offset moves the pattern off the edge; make it smaller.",
        )),
        Err(_) => Err(Issue::new("target", "Window too small for the sample pattern.")),
    }
}

/// The warning when an exclude color cuts into an include range (§5.4), from the compiled table.
pub fn exclusion_overlap(stats: &LutStats) -> Option<Issue> {
    (stats.overlap > 0).then(|| {
        Issue::new(
            "color.exclude",
            "A Never-match color overlaps the color range; those shades won't count.",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{ColorModel, Rgb};
    use crate::profile::KeySpec;
    use crate::profile::tests::appendix_b_profile;

    fn appendix_b() -> Profile {
        Profile::load(appendix_b_profile()).unwrap().value
    }

    fn fields(issues: &[Issue]) -> Vec<&str> {
        issues.iter().map(|i| i.field.as_str()).collect()
    }

    type Change = Box<dyn FnOnce(&mut Profile)>;

    /// The fields one change to Appendix B's profile makes invalid.
    fn errors_after(change: impl FnOnce(&mut Profile)) -> Vec<String> {
        let mut p = appendix_b();
        change(&mut p);
        profile(&p).errors.into_iter().map(|i| i.field).collect()
    }

    #[test]
    fn appendix_b_is_valid_without_warnings() {
        let r = profile(&appendix_b());
        assert_eq!(r, Report::default());
        assert!(settings(&Settings::default()).is_ok());
    }

    #[test]
    fn profile_errors_name_their_field() {
        let cases: Vec<(Vec<&str>, Change)> = vec![
            (vec!["schema_version"], Box::new(|p| p.schema_version = 2)),
            (vec!["name"], Box::new(|p| p.name = "  ".into())),
            (vec!["name"], Box::new(|p| p.name = "x".repeat(65))),
            (vec!["name"], Box::new(|p| p.name = "a\nb".into())),
            (vec!["target.monitor"], Box::new(|p| p.target.monitor.clear())),
            (vec!["target.offset.y"], Box::new(|p| p.target.offset.y = -20_000)),
            (vec!["sampling.k_enter"], Box::new(|p| p.sampling.k_enter = Some(10))),
            (vec!["sampling.k_stay"], Box::new(|p| p.sampling.k_stay = Some(6))),
            (
                vec!["sampling.plus_distance"],
                Box::new(|p| p.sampling.plus_distance = Some(0)),
            ),
            (
                vec!["sampling.offsets"],
                Box::new(|p| {
                    p.sampling.pattern = PatternKind::Custom;
                    p.sampling.k_enter = None;
                    p.sampling.k_stay = None;
                    p.sampling.offsets = vec![[0, 0], [40, 0]];
                }),
            ),
            (
                vec!["sampling.speed"],
                Box::new(|p| p.sampling.speed = Speed::PerSecond(50)),
            ),
            (
                vec!["sampling.speed"],
                Box::new(|p| p.sampling.speed = Speed::PerSecond(20_000)),
            ),
            (vec!["color.exit_margin"], Box::new(|p| p.color.exit_margin = 0.9)),
            (vec!["color.exit_margin"], Box::new(|p| p.color.exit_margin = f64::NAN)),
            (
                vec!["color.include[0].tol_l"],
                Box::new(|p| p.color.include[0].tol_l = Some(0.0)),
            ),
            (
                vec!["color.include[0].tol_h"],
                Box::new(|p| p.color.include[0].tol_h = Some(181.0)),
            ),
            (
                vec!["color.exclude[0].tol_c"],
                Box::new(|p| p.color.exclude[0].tol_c = Some(f64::INFINITY)),
            ),
            (
                vec!["color.include[0].tol_s"],
                Box::new(|p| p.color.include[0].tol_s = Some(-0.1)),
            ),
            (
                vec!["color.include"],
                Box::new(|p| p.color.include = vec![ColorEntry::new(Rgb::new(9, 9, 99)); 17]),
            ),
            (
                vec!["trigger.confirm_samples"],
                Box::new(|p| p.trigger.confirm_samples = 0),
            ),
            (
                vec!["trigger.confirm_min_ms"],
                Box::new(|p| p.trigger.confirm_min_ms = 51),
            ),
            (
                vec!["trigger.rearm_absent_ms"],
                Box::new(|p| p.trigger.rearm_absent_ms = 2001),
            ),
            (
                vec!["trigger.rearm_absent_samples"],
                Box::new(|p| p.trigger.rearm_absent_samples = 101),
            ),
            (
                vec!["trigger.min_fire_interval_ms"],
                Box::new(|p| p.trigger.min_fire_interval_ms = 1001),
            ),
            (
                vec!["trigger.max_clicks_per_sec"],
                Box::new(|p| p.trigger.max_clicks_per_sec = 0),
            ),
            (vec!["action.key"], Box::new(|p| p.action.kind = ActionKind::Key)),
            (
                vec!["action.key.keycode", "action.key.name"],
                Box::new(|p| {
                    p.action.kind = ActionKind::Key;
                    p.action.key = Some(KeySpec {
                        keycode: 7,
                        name: String::new(),
                    });
                }),
            ),
            (vec!["action.hold_ms"], Box::new(|p| p.action.hold_ms = 201)),
            (
                vec!["action.modifiers"],
                Box::new(|p| p.action.modifiers = vec![crate::profile::Modifier::Alt, crate::profile::Modifier::Alt]),
            ),
        ];
        for (expected, change) in cases {
            assert_eq!(errors_after(change), expected);
        }
    }

    #[test]
    fn tolerance_limits_are_inclusive() {
        assert!(
            errors_after(|p| {
                let e = &mut p.color.include[0];
                (e.tol_l, e.tol_c, e.tol_h, e.tol_s, e.tol_v, e.tol_rgb) =
                    (Some(1.0), Some(0.5), Some(180.0), Some(1.0), Some(1.0), Some(0));
                p.color.exit_margin = 3.0;
            })
            .is_empty()
        );
    }

    #[test]
    fn warnings() {
        let mut p = appendix_b();
        p.trigger.rearm_absent_ms = 10;
        p.sampling.speed = Speed::Max;
        p.color.include.push(ColorEntry::new(Rgb::new(0x80, 0x80, 0x80)));
        p.action.hold_ms = 50;
        let r = profile(&p);
        assert!(r.is_ok(), "{:?}", r.errors);
        assert_eq!(
            fields(&r.warnings),
            [
                "sampling.speed",
                "color.include[1].hex",
                "trigger.rearm_absent_ms",
                "trigger.min_fire_interval_ms"
            ]
        );
        assert_eq!(
            r.warnings[3].message,
            "Clicks stay at least 51 ms apart, because each is held 50 ms."
        );
        p.color.model = ColorModel::Rgb; // no hue, so no gray warning
        assert_eq!(profile(&p).warnings.len(), 3);
    }

    #[test]
    fn a_new_profile_is_valid_but_cant_start() {
        let p = Profile::new("Fresh");
        assert!(profile(&p).is_ok());
        assert_eq!(p.start_blocker(), Some("Pick a color to start"));
    }

    #[test]
    fn settings_rules() {
        let errors = |change: fn(&mut Settings)| {
            let mut s = Settings::default();
            change(&mut s);
            settings(&s).errors
        };
        assert_eq!(
            fields(&errors(|s| s.hotkeys.emergency_stop.clear())),
            ["hotkeys.emergency_stop"]
        );
        assert!(errors(|s| s.hotkeys.sample_center.clear()).is_empty()); // unbound is fine
        let dup = errors(|s| s.hotkeys.sample_center = "f7".into());
        assert_eq!(fields(&dup), ["hotkeys.sample_center"]);
        assert_eq!(dup[0].message, "f7 is already the start/stop key.");
        assert!(errors(|s| s.hotkeys.sample_center = "Shift+F7".into()).is_empty());
        let blocked = errors(|s| s.hotkeys.start_stop = "Ctrl+Alt+F2".into());
        assert_eq!(blocked[0].message, "Ctrl+Alt+F2 is reserved by the system.");
        assert_eq!(
            fields(&errors(|s| s.hotkeys.start_stop = "Ctrl+Alt+Backspace".into())),
            ["hotkeys.start_stop"]
        );
        assert_eq!(
            fields(&errors(|s| s.hotkeys.start_stop = "F7+".into())),
            ["hotkeys.start_stop"]
        );
        assert_eq!(fields(&errors(|s| s.schema_version = 0)), ["schema_version"]);
        assert_eq!(fields(&errors(|s| s.active_profile.clear())), ["active_profile"]);
    }

    #[test]
    fn the_action_key_cant_be_a_hotkey() {
        let mut a = Action {
            kind: ActionKind::Key,
            key: Some(KeySpec {
                keycode: 73,
                name: "F7".into(),
            }),
            ..Action::default()
        };
        let h = Hotkeys::default();
        let r = action_against_hotkeys(&a, &h);
        assert_eq!(
            r.errors,
            vec![Issue::new("action.key", "F7 is also the start/stop hotkey.")]
        );
        a.key = Some(KeySpec {
            keycode: 26,
            name: "e".into(),
        });
        assert!(action_against_hotkeys(&a, &h).is_ok());
        a.kind = ActionKind::Mouse;
        a.key = Some(KeySpec {
            keycode: 74,
            name: "f8".into(),
        }); // kept, but unused by a mouse action
        assert!(action_against_hotkeys(&a, &h).is_ok());
        // A side button can't be both the click and a hotkey.
        let side = Hotkeys {
            start_stop: "Mouse4".into(),
            ..Hotkeys::default()
        };
        a.button = MouseButton::Back;
        assert_eq!(
            action_against_hotkeys(&a, &side).errors,
            vec![Issue::new("action.key", "Mouse4 is also the start/stop hotkey.")]
        );
        a.button = MouseButton::Forward;
        assert!(action_against_hotkeys(&a, &side).is_ok());
    }

    #[test]
    fn placement_and_overlap() {
        let pattern = appendix_b().pattern().unwrap();
        let monitor = Rect::new(0, 0, 1920, 1080);
        assert_eq!(placement(&pattern, monitor, (0, 0)), Ok(Rect::new(959, 539, 3, 3)));
        assert_eq!(
            placement(&pattern, monitor, (960, 0)).unwrap_err().field,
            "target.offset"
        );
        assert_eq!(
            placement(&pattern, Rect::new(0, 0, 2, 2), (0, 0)).unwrap_err().field,
            "target"
        );
        assert_eq!(exclusion_overlap(&LutStats::default()), None);
        let overlap = LutStats {
            overlap: 3,
            ..LutStats::default()
        };
        assert_eq!(exclusion_overlap(&overlap).unwrap().field, "color.exclude");
    }
}
