// SPDX-License-Identifier: MIT
//! The scenarios of §16.
//!
//! Most of them are a [`Run`]: the lab draws on the canvas while the probe and the trace record,
//! and [`judge`] decides afterwards. The rest check one mechanism directly: a second engine,
//! the safety stop, the action's input sequence, the release after a crash, and validation.

use std::time::{Duration, Instant};

use glint_core::color::{ColorEntry, ColorModel, CompiledRules, ShadePreset};
use glint_core::pattern::Rect;
use glint_core::profile::{Action, ActionKind, KeySpec, Modifier, MouseButton, TargetMode};
use glint_core::settings::Settings;
use glint_core::{Class, Profile, Rgb};
use glint_ipc::{Event, MAX_CLASSIFY, Request, StopCause};
use glint_x11::keymap::{Keymap, keysym, keysym_from_name};
use rustix::process::Signal;

use crate::canvas::{Area, BACKGROUND, Canvas, Drawn, GREEN, Input, InputKind, PATCH};
use crate::host::Host;
use crate::judge::{Change, Evidence, JudgeCfg, Mark, Report, Shown, judge};
use crate::probe::Probe;
use crate::trace::TraceTap;
use crate::{LabError, Session};

/// A second color for profile edits: a blue, far from the green.
const BLUE: u32 = 0x3B_6F_D1;
/// One frame at 60 Hz.
const FRAME: Duration = Duration::from_micros(16_667);
/// `Button1Mask` in the core protocol's pointer state.
const BUTTON1_MASK: u16 = 1 << 8;

/// One scenario of §16.
pub struct Scenario {
    pub name: &'static str,
    /// What it shows and what passing means.
    pub about: &'static str,
    /// The count when none is given: appearances, flashes or rounds.
    pub count: u32,
    /// Why it runs only in the sandbox, if it does.
    pub sandbox_only: Option<&'static str>,
    pub run: fn(&mut Session) -> Result<Report, LabError>,
}

pub const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "basic",
        about: "the exact color, on for 30–500 ms and off for 60–600 ms: one click per appearance",
        count: 100,
        sandbox_only: None,
        run: basic,
    },
    Scenario {
        name: "shades",
        about: "random shades inside the tight range: one click per appearance",
        count: 100,
        sandbox_only: None,
        run: shades,
    },
    Scenario {
        name: "near-miss",
        about: "colors just outside the wide range, and an exclude color: no clicks",
        count: 100,
        sandbox_only: None,
        run: near_miss,
    },
    Scenario {
        name: "distractor",
        about: "the color right next to the sampled pixels but never on them: no clicks for those",
        count: 100,
        sandbox_only: None,
        run: distractor,
    },
    Scenario {
        name: "flicker",
        about: "1–2 frame dropouts inside appearances: one click per appearance",
        count: 50,
        sandbox_only: None,
        run: flicker,
    },
    Scenario {
        name: "boundary-noise",
        about: "the color jitters between the tight and the wide range: one click per appearance",
        count: 50,
        sandbox_only: None,
        run: boundary_noise,
    },
    Scenario {
        name: "fade",
        about: "300 ms fades in and out through the shades in between: one click per appearance",
        count: 30,
        sandbox_only: None,
        run: fade,
    },
    Scenario {
        name: "rapid",
        about: "gaps just above Gone after (a click each), then just below (one click for the merged run)",
        count: 10,
        sandbox_only: None,
        run: rapid,
    },
    Scenario {
        name: "arm-while-present",
        about: "Start while the color shows: no click until it leaves and comes back",
        count: 20,
        sandbox_only: None,
        run: arm_while_present,
    },
    Scenario {
        name: "live-edit",
        about: "profile and range changes while watching: no click caused by an edit",
        count: 10,
        sandbox_only: None,
        run: live_edit,
    },
    Scenario {
        name: "move",
        about: "the target window moves, or resizes so the color lands under its new center: no click caused by it",
        count: 8,
        sandbox_only: Some("it moves windows around"),
        run: moving,
    },
    Scenario {
        name: "focus",
        about: "another window takes the focus mid-appearance, then the target gets it back with the color showing: no click into the other window, none on refocus",
        count: 8,
        sandbox_only: Some("it moves the focus between windows"),
        run: focus,
    },
    Scenario {
        name: "ui-overlap",
        about: "a Glint window covers the watched spot: a warning, no live swatch while covered, so no click it could cause",
        count: 3,
        sandbox_only: None,
        run: ui_overlap,
    },
    Scenario {
        name: "stall",
        about: "the engine frozen (SIGSTOP) for 30 ms around edges: no extra click; a miss only if the engine reported it",
        count: 50,
        sandbox_only: None,
        run: stall,
    },
    Scenario {
        name: "two-engines",
        about: "a second glintd on the same display refuses to start (exit code 3)",
        count: 1,
        sandbox_only: Some("it starts a second engine; the sandbox shows the same thing"),
        run: two_engines,
    },
    Scenario {
        name: "feedback",
        about: "the screen answers every click with a new appearance: a safety stop within 1 s",
        count: 3,
        sandbox_only: None,
        run: feedback,
    },
    Scenario {
        name: "action-matrix",
        about: "every mouse button and a set of keys, held 0/10/50 ms: one press, then one release, each",
        count: 1,
        sandbox_only: Some("it presses keys, which would reach whatever window has the focus"),
        run: action_matrix,
    },
    Scenario {
        name: "kill-mid-press",
        about: "SIGKILL the engine while it holds input, then restart it: everything released",
        count: 2,
        sandbox_only: Some("it leaves input held on purpose until the engine restarts"),
        run: kill_mid_press,
    },
    Scenario {
        name: "action-equals-hotkey",
        about: "an action key that is also a hotkey is rejected, both ways round",
        count: 1,
        sandbox_only: Some("it checks validation only"),
        run: action_equals_hotkey,
    },
    Scenario {
        name: "soak",
        about: "the scenarios that show colors, mixed at random, for --minutes (default 60): 0 violations, flat memory",
        count: 1,
        sandbox_only: None,
        run: soak,
    },
];

/// The §16 scenarios that need later phases.
pub const LATER: &[(&str, &str)] = &[];

/// The lab's profile: the green, Normal shades, everything else default.
pub fn lab_profile() -> Profile {
    let mut p = Profile::new("glint-lab");
    p.color.include = vec![ShadePreset::Normal.entry(ColorModel::Oklch, Rgb::from_u32(GREEN))];
    p
}

/// The colors `near-miss` shows: shades just outside the wide range, a neighboring hue, and an
/// exclude color inside the include range.
pub fn near_miss_colors() -> [u32; 4] {
    [
        0x26_92_4C, // lightness 0.178 below the green: outside the wide range (0.15)
        0x6B_D1_3B, // yellow-green, 12.8° of hue away: outside even the wide range (10.5°)
        0x2D_D6_C0, // a greenish cyan, far in hue
        0x40_D5_75, // inside the include range, but listed as an exclusion
    ]
}

/// The exclusion `near-miss` adds, around its last color.
pub fn near_miss_exclusion() -> ColorEntry {
    let mut e = ColorEntry::new(Rgb::from_u32(0x40_D5_75));
    e.tol_l = Some(0.01);
    e.tol_c = Some(0.01);
    e.tol_h = Some(1.0);
    e
}

/// A small deterministic generator for the scenarios' timings.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number below `n`.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    /// A duration between `lo` and `hi` milliseconds.
    pub fn ms(&mut self, lo: u64, hi: u64) -> Duration {
        Duration::from_millis(lo + self.below(hi - lo + 1))
    }

    pub fn coin(&mut self) -> bool {
        self.next() & 1 == 1
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

fn sleep(d: Duration) {
    std::thread::sleep(d);
}

/// The engine applies a Start, Stop or profile change on the hot thread's next loop, shortly
/// after its reply.
fn settle(at: Drawn) -> Drawn {
    Drawn {
        sent: at.sent,
        drawn: at.drawn + Duration::from_millis(2),
    }
}

/// One judged run: the probe and the trace for its whole length, and every mark the lab made.
struct Run {
    marks: Vec<Mark>,
    rules: CompiledRules,
    showing: u32,
    started: bool,
    inputs: Vec<Input>,
    probe: Probe,
    tap: TraceTap,
    cfg: JudgeCfg,
}

impl Run {
    /// Sets `profile` and starts the probe and the trace, after `baseline` of quiet for the
    /// server tax. Watching starts with [`Run::start`].
    fn begin(s: &mut Session, profile: &Profile, baseline: Duration) -> Result<Self, LabError> {
        s.set_profile(profile)?;
        s.refresh_backend()?;
        let clear = s.canvas.clear()?;
        let probe = Probe::start(s.host.display())?;
        sleep(baseline);
        let tap = TraceTap::connect(s.host.socket())?;
        s.canvas.take_inputs();
        let mut cfg = JudgeCfg::new(
            Duration::from_millis(u64::from(profile.trigger.rearm_absent_ms)),
            profile.sampling.speed.period(),
        );
        // The engine's own check gap, longer under a compositor (ADR 0007).
        let gap = s.state()?.check_gap_us;
        if gap > 0 {
            cfg.slack = Duration::from_micros(u64::from(gap)) * 2;
        }
        Ok(Self {
            marks: vec![Mark {
                at: clear,
                shown: Shown::Absent,
                change: Change::Draw,
            }],
            rules: CompiledRules::new(&profile.color),
            showing: BACKGROUND,
            started: false,
            inputs: Vec::new(),
            probe,
            tap,
            cfg,
        })
    }

    fn mark(&mut self, at: Drawn, color: u32, change: Change) {
        self.showing = color;
        let shown = self.rules.classify(Rgb::from_u32(color)).into();
        self.marks.push(Mark { at, shown, change });
    }

    fn show(&mut self, s: &Session, color: u32) -> Result<(), LabError> {
        let at = s.canvas.show(color)?;
        self.mark(at, color, Change::Draw);
        Ok(())
    }

    fn clear(&mut self, s: &Session) -> Result<(), LabError> {
        let at = s.canvas.clear()?;
        self.mark(at, BACKGROUND, Change::Draw);
        Ok(())
    }

    /// Draws `color` where the pattern doesn't look: the watched spot keeps what it showed.
    fn beside(&mut self, s: &Session, color: u32, areas: &[Area]) -> Result<(), LabError> {
        let at = s.canvas.fill(color, areas)?;
        self.mark(at, self.showing, Change::Draw);
        Ok(())
    }

    fn start(&mut self, s: &mut Session) -> Result<(), LabError> {
        let at = settle(s.start()?);
        self.probe.watching();
        self.started = true;
        self.mark(at, self.showing, Change::Restart);
        Ok(())
    }

    fn stop(&mut self, s: &mut Session) -> Result<(), LabError> {
        let at = settle(s.stop()?);
        self.started = false;
        self.mark(at, self.showing, Change::Stop);
        Ok(())
    }

    /// A profile change: while watching, the engine restarts (latched) under the new rules.
    fn edit(&mut self, s: &mut Session, profile: &Profile) -> Result<(), LabError> {
        let sent = Instant::now();
        s.set_profile(profile)?;
        let at = settle(Drawn {
            sent,
            drawn: Instant::now(),
        });
        self.rules = CompiledRules::new(&profile.color);
        let change = if self.started { Change::Restart } else { Change::Draw };
        self.mark(at, self.showing, change);
        Ok(())
    }

    /// The next input on the canvas, kept for the judgment.
    fn next_input(&mut self, s: &Session, timeout: Duration) -> Option<Input> {
        let i = s.canvas.next_input(timeout)?;
        self.inputs.push(i);
        Some(i)
    }

    /// Clears the screen, lets the last click arrive, stops, and judges.
    fn finish(mut self, s: &mut Session, title: String, timing: bool) -> Result<Report, LabError> {
        self.clear(s)?;
        sleep(Duration::from_millis(200));
        let end = Instant::now();
        if self.started {
            s.stop()?;
        }
        self.inputs.extend(s.canvas.take_inputs());
        let trace = self.tap.finish();
        let probe = self.probe.finish();
        let cfg = JudgeCfg {
            timing: timing && s.host.is_real(),
            ..self.cfg
        };
        let mut r = judge(
            title,
            &cfg,
            &Evidence {
                marks: &self.marks,
                end,
                inputs: &self.inputs,
                trace: &trace,
                probe: Some(&probe),
            },
        );
        r.sandbox = !s.host.is_real();
        let gap = self.cfg.slack / 2;
        let ms = gap.as_secs_f64() * 1e3;
        if s.tags().contains("damage") {
            r.notes.push(format!(
                "checks when the screen reports a change, and every 10 ms: check gap {ms:.1} ms (ADR 0018)"
            ));
        } else if gap > Duration::from_millis(2) {
            r.notes
                .push(format!("the engine saw a compositor: check gap {ms:.1} ms (ADR 0007)"));
        }
        Ok(r)
    }
}

/// The engine's own lookup table must agree on `colors`.
fn expect_classes(s: &mut Session, colors: &[u32], ok: impl Fn(Class) -> bool, what: &str) -> Result<(), LabError> {
    for chunk in colors.chunks(MAX_CLASSIFY) {
        let request = Request::Classify {
            colors: chunk.iter().map(|&c| Rgb::from_u32(c)).collect(),
        };
        if let Event::Classified { verdicts } = s.call(request)?
            && let Some((c, v)) = chunk.iter().zip(&verdicts).find(|(_, v)| !ok(**v))
        {
            return Err(LabError::Setup(format!("#{c:06X} is {v:?} to the engine, not {what}")));
        }
    }
    Ok(())
}

/// `a` blended toward `b` by `k / n`, channel by channel.
fn mix(a: u32, b: u32, k: u32, n: u32) -> u32 {
    let ch = |shift: u32| {
        let (x, y) = (((a >> shift) & 0xFF) as i64, ((b >> shift) & 0xFF) as i64);
        ((x + (y - x) * i64::from(k) / i64::from(n.max(1))) as u32) << shift
    };
    ch(16) | ch(8) | ch(0)
}

/// Random shades inside the tight range, other than the green itself.
fn tight_shades(rules: &CompiledRules, rng: &mut Rng, n: usize) -> Vec<u32> {
    let mut pool = Vec::new();
    for _ in 0..200_000 {
        if pool.len() == n {
            break;
        }
        let mut c = 0;
        for shift in [16, 8, 0] {
            let v = ((GREEN >> shift) & 0xFF) as i64 + rng.below(97) as i64 - 48;
            c |= (v.clamp(0, 255) as u32) << shift;
        }
        if c != GREEN && !pool.contains(&c) && rules.classify(Rgb::from_u32(c)) == Class::Tight {
            pool.push(c);
        }
    }
    pool
}

/// A color in the wide range but not the tight one: the green darkened past the tight range,
/// from the middle of the stretch that is only wide.
fn wide_only(rules: &CompiledRules) -> Option<u32> {
    let wide: Vec<u32> = (0..=100)
        .map(|k| mix(GREEN, BACKGROUND, k, 100))
        .filter(|&c| rules.classify(Rgb::from_u32(c)) == Class::Wide)
        .collect();
    wide.get(wide.len() / 2).copied()
}

/// Shapes in the green right next to the pattern's pixels, never on them: everything in the
/// patch but those pixels, a ring one pixel away, and blocks on each side.
fn distractor_shapes(s: &Session, profile: &Profile) -> Result<Vec<Vec<Area>>, LabError> {
    let pattern = profile.pattern().map_err(|e| LabError::Setup(e.to_string()))?;
    let (ax, ay) = s.canvas.anchor();
    let px: Vec<(i32, i32)> = pattern
        .points()
        .map(|(dx, dy)| (ax + i32::from(dx), ay + i32::from(dy)))
        .collect();
    let x0 = px.iter().map(|p| p.0).min().unwrap_or(ax);
    let x1 = px.iter().map(|p| p.0).max().unwrap_or(ax);
    let y0 = px.iter().map(|p| p.1).min().unwrap_or(ay);
    let y1 = px.iter().map(|p| p.1).max().unwrap_or(ay);
    let (w, h) = ((x1 - x0 + 1) as u16, (y1 - y0 + 1) as u16);
    let (px0, py0, pw, ph) = s.canvas.square(PATCH);
    let mut holes = Vec::new();
    for y in py0..py0 + i32::from(ph) {
        let mut cols: Vec<i32> = px.iter().filter(|p| p.1 == y).map(|p| p.0).collect();
        cols.sort_unstable();
        let mut x = px0;
        for c in cols {
            if c > x {
                holes.push((x, y, (c - x) as u16, 1));
            }
            x = c + 1;
        }
        let end = px0 + i32::from(pw);
        if end > x {
            holes.push((x, y, (end - x) as u16, 1));
        }
    }
    let ring = vec![
        (x0 - 11, y0 - 11, w + 22, 10),
        (x0 - 11, y1 + 2, w + 22, 10),
        (x0 - 11, y0 - 1, 10, h + 2),
        (x1 + 2, y0 - 1, 10, h + 2),
    ];
    Ok(vec![
        holes,
        ring,
        vec![(x1 + 2, y0 - 20, 40, h + 40)],
        vec![(x0 - 41, y0 - 20, 40, h + 40)],
        vec![(x0 - 20, y0 - 41, w + 40, 40)],
        vec![(x0 - 20, y1 + 2, w + 40, 40)],
    ])
}

/// Off for a while, then `color` for a while.
fn appear(
    run: &mut Run,
    s: &Session,
    rng: &mut Rng,
    color: u32,
    off: (u64, u64),
    on: (u64, u64),
) -> Result<(), LabError> {
    run.clear(s)?;
    sleep(rng.ms(off.0, off.1));
    run.show(s, color)?;
    sleep(rng.ms(on.0, on.1));
    Ok(())
}

/// An appearance with one to three dropouts of one or two frames.
fn flicker_once(run: &mut Run, s: &Session, rng: &mut Rng) -> Result<(), LabError> {
    run.clear(s)?;
    sleep(rng.ms(80, 300));
    run.show(s, GREEN)?;
    sleep(rng.ms(40, 120));
    for _ in 0..1 + rng.below(3) {
        run.clear(s)?;
        sleep(if rng.coin() { FRAME } else { FRAME * 2 });
        run.show(s, GREEN)?;
        sleep(rng.ms(40, 120));
    }
    Ok(())
}

/// An appearance that jitters between the green and `wide` every 5–20 ms.
fn boundary_once(run: &mut Run, s: &Session, rng: &mut Rng, wide: u32) -> Result<(), LabError> {
    run.clear(s)?;
    sleep(rng.ms(80, 300));
    let until = Instant::now() + rng.ms(150, 400);
    let mut tight = rng.coin();
    while Instant::now() < until {
        run.show(s, if tight { GREEN } else { wide })?;
        sleep(rng.ms(5, 20));
        tight = !tight;
    }
    Ok(())
}

/// A 300 ms fade in, a hold, and a 300 ms fade out, in 15 ms steps.
fn fade_once(run: &mut Run, s: &Session, rng: &mut Rng) -> Result<(), LabError> {
    run.clear(s)?;
    sleep(rng.ms(100, 400));
    for k in 1..=20 {
        run.show(s, mix(BACKGROUND, GREEN, k, 20))?;
        sleep(Duration::from_millis(15));
    }
    sleep(rng.ms(100, 300));
    for k in (0..20).rev() {
        run.show(s, mix(BACKGROUND, GREEN, k, 20))?;
        sleep(Duration::from_millis(15));
    }
    Ok(())
}

/// Five appearances with gaps 15 ms past where a re-arm is certain (*Gone after* plus two check
/// gaps), then five with gaps 15 ms shorter than *Gone after*.
fn rapid_round(run: &mut Run, s: &Session, rng: &mut Rng) -> Result<(), LabError> {
    let gone = run.cfg.gone_after;
    let margin = Duration::from_millis(15);
    run.clear(s)?;
    sleep(Duration::from_millis(300));
    for gap in [gone + run.cfg.slack + margin, gone.saturating_sub(margin)] {
        for _ in 0..5 {
            run.show(s, GREEN)?;
            sleep(rng.ms(40, 80));
            run.clear(s)?;
            sleep(gap);
        }
        sleep(Duration::from_millis(300));
    }
    Ok(())
}

/// The profile `live-edit` and `soak` switch to: the blue instead of the green.
fn blue_profile(base: &Profile, shades: ShadePreset) -> Profile {
    let mut p = base.clone();
    p.color.include = vec![shades.entry(ColorModel::Oklch, Rgb::from_u32(BLUE))];
    p
}

fn basic(s: &mut Session) -> Result<Report, LabError> {
    let n = s.count(100);
    let title = s.title("basic", n);
    let mut rng = Rng::new(s.opts.seed);
    let profile = s.profile();
    let mut run = Run::begin(s, &profile, Duration::from_secs(1))?;
    run.start(s)?;
    for _ in 0..n {
        appear(&mut run, s, &mut rng, GREEN, (60, 600), (30, 500))?;
    }
    run.finish(s, title, true)
}

fn shades(s: &mut Session) -> Result<Report, LabError> {
    let n = s.count(100);
    let title = s.title("shades", n);
    let mut rng = Rng::new(s.opts.seed);
    let profile = s.profile();
    let pool = tight_shades(&CompiledRules::new(&profile.color), &mut rng, 64);
    if pool.len() < 16 {
        return Err(LabError::Setup(format!("only {} tight shades found", pool.len())));
    }
    let mut run = Run::begin(s, &profile, Duration::ZERO)?;
    expect_classes(s, &pool, |c| c == Class::Tight, "tight")?;
    run.start(s)?;
    for _ in 0..n {
        let color = *rng.pick(&pool);
        appear(&mut run, s, &mut rng, color, (60, 400), (30, 300))?;
    }
    let mut r = run.finish(s, title, true)?;
    r.notes.push(format!("{} different shades", pool.len()));
    Ok(r)
}

fn near_miss(s: &mut Session) -> Result<Report, LabError> {
    let n = s.count(100);
    let title = s.title("near-miss", n);
    let mut rng = Rng::new(s.opts.seed);
    let mut profile = s.profile();
    profile.color.exclude = vec![near_miss_exclusion()];
    let colors = near_miss_colors();
    let mut run = Run::begin(s, &profile, Duration::ZERO)?;
    expect_classes(
        s,
        &colors,
        |c| !matches!(c, Class::Tight | Class::Wide),
        "outside the ranges",
    )?;
    run.start(s)?;
    for i in 0..n as usize {
        appear(&mut run, s, &mut rng, colors[i % colors.len()], (60, 200), (50, 300))?;
    }
    run.finish(s, title, false)
}

fn distractor(s: &mut Session) -> Result<Report, LabError> {
    let n = s.count(100);
    let title = s.title("distractor", n);
    let mut rng = Rng::new(s.opts.seed);
    let profile = s.profile();
    let shapes = distractor_shapes(s, &profile)?;
    let mut run = Run::begin(s, &profile, Duration::ZERO)?;
    run.start(s)?;
    for i in 0..n as usize {
        run.clear(s)?;
        sleep(rng.ms(60, 200));
        // Every fifth flash is a real appearance, which shows the engine is watching.
        if i % 5 == 4 {
            run.show(s, GREEN)?;
        } else {
            run.beside(s, GREEN, &shapes[i % shapes.len()])?;
        }
        sleep(rng.ms(50, 300));
    }
    let mut r = run.finish(s, title, false)?;
    r.notes.push(format!(
        "{} distractors, {} real appearances",
        n as usize - n as usize / 5,
        n / 5
    ));
    Ok(r)
}

fn flicker(s: &mut Session) -> Result<Report, LabError> {
    let n = s.count(50);
    let title = s.title("flicker", n);
    let mut rng = Rng::new(s.opts.seed);
    let profile = s.profile();
    let mut run = Run::begin(s, &profile, Duration::ZERO)?;
    run.start(s)?;
    for _ in 0..n {
        flicker_once(&mut run, s, &mut rng)?;
    }
    run.finish(s, title, false)
}

fn boundary_noise(s: &mut Session) -> Result<Report, LabError> {
    let n = s.count(50);
    let title = s.title("boundary-noise", n);
    let mut rng = Rng::new(s.opts.seed);
    let profile = s.profile();
    let wide = wide_only(&CompiledRules::new(&profile.color))
        .ok_or_else(|| LabError::Setup("no color is only in the wide range".into()))?;
    let mut run = Run::begin(s, &profile, Duration::ZERO)?;
    expect_classes(s, &[wide], |c| c == Class::Wide, "wide only")?;
    run.start(s)?;
    for _ in 0..n {
        boundary_once(&mut run, s, &mut rng, wide)?;
    }
    let mut r = run.finish(s, title, false)?;
    r.notes.push(format!("jitters between #{GREEN:06X} and #{wide:06X}"));
    Ok(r)
}

fn fade(s: &mut Session) -> Result<Report, LabError> {
    let n = s.count(30);
    let title = s.title("fade", n);
    let mut rng = Rng::new(s.opts.seed);
    let profile = s.profile();
    let mut run = Run::begin(s, &profile, Duration::ZERO)?;
    run.start(s)?;
    for _ in 0..n {
        fade_once(&mut run, s, &mut rng)?;
    }
    run.finish(s, title, false)
}

fn rapid(s: &mut Session) -> Result<Report, LabError> {
    let n = s.count(10);
    let title = s.title("rapid", n);
    let mut rng = Rng::new(s.opts.seed);
    let profile = s.profile();
    let mut run = Run::begin(s, &profile, Duration::ZERO)?;
    run.start(s)?;
    for _ in 0..n {
        rapid_round(&mut run, s, &mut rng)?;
    }
    let mut r = run.finish(s, title, false)?;
    r.notes.push(
        "per round: 5 appearances whose gaps clear Gone after and two check gaps (a click each), then 5 whose gaps are 15 ms short of Gone after (one click)".into(),
    );
    Ok(r)
}

fn arm_while_present(s: &mut Session) -> Result<Report, LabError> {
    let n = s.count(20);
    let title = s.title("arm-while-present", n);
    let profile = s.profile();
    let mut run = Run::begin(s, &profile, Duration::ZERO)?;
    for _ in 0..n {
        run.show(s, GREEN)?;
        run.start(s)?;
        sleep(Duration::from_millis(250));
        run.clear(s)?;
        sleep(Duration::from_millis(100));
        run.show(s, GREEN)?;
        sleep(Duration::from_millis(150));
        run.stop(s)?;
        run.clear(s)?;
        sleep(Duration::from_millis(50));
    }
    run.finish(s, title, false)
}

fn live_edit(s: &mut Session) -> Result<Report, LabError> {
    let n = s.count(10);
    let title = s.title("live-edit", n);
    let mut rng = Rng::new(s.opts.seed);
    let green = s.profile();
    let blue = blue_profile(&green, ShadePreset::Normal);
    let blue_wider = blue_profile(&green, ShadePreset::Loose);
    let mut run = Run::begin(s, &green, Duration::ZERO)?;
    run.start(s)?;
    for _ in 0..n {
        // An edit while nothing shows, then the new color: a click.
        run.clear(s)?;
        sleep(rng.ms(80, 150));
        run.edit(s, &blue)?;
        sleep(rng.ms(100, 200));
        run.show(s, BLUE)?;
        sleep(rng.ms(100, 200));
        // The range moves off the showing color and back: the restart latches, so no click.
        run.edit(s, &green)?;
        sleep(rng.ms(60, 120));
        run.edit(s, &blue)?;
        sleep(rng.ms(80, 150));
        // The range widens around the showing color: no click.
        run.edit(s, &blue_wider)?;
        sleep(rng.ms(80, 150));
        appear(&mut run, s, &mut rng, BLUE, (80, 150), (100, 200))?;
        // Back to the green while the blue shows, then the green: a click.
        run.edit(s, &green)?;
        sleep(rng.ms(60, 120));
        appear(&mut run, s, &mut rng, GREEN, (80, 150), (100, 200))?;
    }
    let mut r = run.finish(s, title, false)?;
    r.notes.push(format!("{} profile edits while watching", n * 5));
    Ok(r)
}

/// The class of the lab's game window, and of the other window `focus` uses.
const GAME: &str = "glint-lab-game";
const OTHER: &str = "glint-lab-other";
/// Where the game window starts.
const HOME: Rect = Rect {
    x: 200,
    y: 150,
    w: 400,
    h: 300,
};

/// The engine follows a window change through an X event and the controller, so a move or a
/// focus change takes effect a little after the request's round trip.
fn settle_window(at: Drawn) -> Drawn {
    Drawn {
        sent: at.sent,
        drawn: at.drawn + Duration::from_millis(20),
    }
}

/// The lab's profile watching the game window's center.
fn window_profile(s: &Session) -> Profile {
    let mut p = s.profile();
    p.target.mode = TargetMode::Window;
    p.target.window.class = GAME.into();
    p.guards.only_in_window = true;
    p
}

/// Runs `body` with a game window as the session's canvas, then puts the full-screen canvas back
/// (it stays mapped underneath, a uniform background).
fn with_game_window(
    s: &mut Session,
    body: impl FnOnce(&mut Session) -> Result<Report, LabError>,
) -> Result<Report, LabError> {
    let game = Canvas::game(s.host.display(), HOME, GAME)?;
    let full = std::mem::replace(&mut s.canvas, game);
    let report = body(s);
    s.canvas = full;
    report
}

fn moving(s: &mut Session) -> Result<Report, LabError> {
    with_game_window(s, |s| {
        let n = s.count(8);
        let mut rng = Rng::new(s.opts.seed);
        let profile = window_profile(s);
        let mut run = Run::begin(s, &profile, Duration::ZERO)?;
        let title = s.title("move", n);
        run.start(s)?;
        for _ in 0..n {
            appear(&mut run, s, &mut rng, GREEN, (100, 200), (100, 200))?;
            // The window moves while the color shows: it moves with the window, and the
            // engine's restart latches.
            let (dx, dy) = (rng.below(120) as i32 - 60, rng.below(80) as i32 - 40);
            let moved = Rect::new(HOME.x + dx, HOME.y + dy, HOME.w, HOME.h);
            let at = s.canvas.set_rect(moved)?;
            run.mark(settle_window(at), GREEN, Change::Restart);
            sleep(rng.ms(120, 200));
            // A patch 40 px right of the center; then the window widens by 80 px, which puts
            // the patch under the new center.
            run.clear(s)?;
            sleep(rng.ms(100, 150));
            let (ax, ay) = s.canvas.anchor();
            let at = s.canvas.fill(GREEN, &[(ax + 30, ay - 10, 20, 20)])?;
            run.mark(at, BACKGROUND, Change::Draw);
            sleep(rng.ms(60, 120));
            let wider = Rect::new(moved.x, moved.y, moved.w + 80, moved.h);
            let at = s.canvas.set_rect(wider)?;
            run.mark(settle_window(at), GREEN, Change::Restart);
            sleep(rng.ms(120, 200));
            // Back home, then a normal appearance clicks.
            run.clear(s)?;
            let at = s.canvas.set_rect(HOME)?;
            run.mark(settle_window(at), BACKGROUND, Change::Restart);
            sleep(rng.ms(120, 200));
            appear(&mut run, s, &mut rng, GREEN, (60, 100), (100, 200))?;
        }
        let mut r = run.finish(s, title, false)?;
        r.notes
            .push(format!("{} moves and {} resizes while watching", n * 2, n));
        Ok(r)
    })
}

fn focus(s: &mut Session) -> Result<Report, LabError> {
    with_game_window(s, |s| {
        let n = s.count(8);
        let mut rng = Rng::new(s.opts.seed);
        let profile = window_profile(s);
        let other = Canvas::game(s.host.display(), Rect::new(700, 150, 300, 200), OTHER)?;
        s.canvas.focus()?;
        s.canvas.warp_inside()?;
        let mut run = Run::begin(s, &profile, Duration::ZERO)?;
        let title = s.title("focus", n);
        run.start(s)?;
        for _ in 0..n {
            appear(&mut run, s, &mut rng, GREEN, (100, 200), (100, 200))?;
            run.clear(s)?;
            sleep(rng.ms(100, 150));
            // Another window takes the focus, with the pointer over it: watching pauses, and a
            // click would land in the wrong window.
            let at = other.focus()?;
            other.warp_inside()?;
            run.mark(settle_window(at), BACKGROUND, Change::Stop);
            sleep(rng.ms(60, 100));
            run.show(s, GREEN)?;
            sleep(rng.ms(120, 200));
            // Back in front with the color showing: the restart latches.
            s.canvas.warp_inside()?;
            let at = s.canvas.focus()?;
            run.mark(settle_window(at), GREEN, Change::Restart);
            sleep(rng.ms(120, 200));
            appear(&mut run, s, &mut rng, GREEN, (100, 150), (100, 200))?;
        }
        let mut r = run.finish(s, title, false)?;
        let strays = other
            .take_inputs()
            .iter()
            .filter(|i| matches!(i.kind, InputKind::ButtonPress(_)))
            .count();
        if strays > 0 {
            r.problems.push(format!("{strays} clicks landed in the other window"));
        }
        r.notes.push(format!("{n} focus changes away and back while watching"));
        Ok(r)
    })
}

/// The next events on `client` for `wait`, each handed to `seen`.
fn events_for(client: &mut glint_ipc::Client, wait: Duration, mut seen: impl FnMut(Event)) {
    let until = Instant::now() + wait;
    while let Some(left) = until.checked_duration_since(Instant::now()) {
        match client.next_event(left.max(Duration::from_millis(1))) {
            Ok(e) => seen(e.body),
            Err(_) => break,
        }
    }
}

fn ui_overlap(s: &mut Session) -> Result<Report, LabError> {
    let n = s.count(3);
    let title = s.title("ui-overlap", n);
    let mut rng = Rng::new(s.opts.seed);
    let profile = s.profile();
    let mut run = Run::begin(s, &profile, Duration::ZERO)?;
    run.start(s)?;
    // A second connection plays the UI: it reports its window and gets the live swatch.
    let mut ui = s.host.client()?;
    s.call(Request::Ping)?;
    ui.call(
        Request::Subscribe {
            telemetry_hz: 0,
            live_hz: 20,
            trace: false,
        },
        crate::WAIT,
    )
    .map_err(crate::engine_err)?;
    let (ax, ay) = s.canvas.anchor();
    let mut problems = Vec::new();
    for round in 0..n {
        // The color shows and gets its click: the swatch says "match".
        appear(&mut run, s, &mut rng, GREEN, (100, 200), (150, 250))?;
        // The UI's window moves over the watched spot.
        ui.send(Request::UiGeometry {
            x: ax - 150,
            y: ay - 100,
            w: 300,
            h: 200,
        })
        .map_err(crate::engine_err)?;
        let mut warned = false;
        events_for(&mut ui, Duration::from_millis(300), |e| {
            warned |= matches!(e, Event::Warning { ref code, .. } if code == "ui_overlap");
        });
        if !warned {
            problems.push(format!(
                "round {}: no warning when the window covered the spot",
                round + 1
            ));
        }
        // Covered, the UI would paint its swatch over the spot: whatever the engine sees, the
        // engine would then see. With the swatch off, nothing gets painted.
        run.clear(s)?;
        let mut live = 0;
        let mut paint = Vec::new();
        events_for(&mut ui, Duration::from_millis(600), |e| {
            if let Event::Live { verdict, .. } = e {
                live += 1;
                paint.push(verdict == glint_core::Verdict::Enter);
            }
        });
        for green in paint {
            if green { run.show(s, GREEN)? } else { run.clear(s)? }
        }
        if live > 0 {
            problems.push(format!("round {}: {live} live swatch updates while covered", round + 1));
        }
        // The window moves away: the swatch comes back.
        ui.send(Request::UiGeometry {
            x: 0,
            y: 0,
            w: 120,
            h: 80,
        })
        .map_err(crate::engine_err)?;
        let mut back = 0;
        events_for(&mut ui, Duration::from_millis(400), |e| {
            back += u32::from(matches!(e, Event::Live { .. }))
        });
        if back == 0 {
            problems.push(format!(
                "round {}: no live swatch after the window moved away",
                round + 1
            ));
        }
    }
    let mut r = run.finish(s, title, false)?;
    r.problems.extend(problems);
    r.notes.push(format!("{n} times covered and uncovered while watching"));
    Ok(r)
}

/// Sends SIGCONT when dropped, so a scenario that fails mid-freeze never leaves the engine stopped.
struct Thaw<'a>(&'a dyn Host);

impl Drop for Thaw<'_> {
    fn drop(&mut self) {
        let _ = self.0.signal(Signal::CONT);
    }
}

/// Draws `color` (the background means clear), half the time with the engine frozen from 15 ms
/// before to 15 ms after. Returns whether it froze the engine.
fn edge(run: &mut Run, s: &Session, rng: &mut Rng, color: u32) -> Result<bool, LabError> {
    let draw = |run: &mut Run| {
        if color == BACKGROUND {
            run.clear(s)
        } else {
            run.show(s, color)
        }
    };
    if !rng.coin() {
        draw(run)?;
        return Ok(false);
    }
    s.host.signal(Signal::STOP)?;
    let _thaw = Thaw(&*s.host);
    sleep(Duration::from_millis(15));
    draw(run)?;
    sleep(Duration::from_millis(15));
    Ok(true)
}

fn stall(s: &mut Session) -> Result<Report, LabError> {
    let n = s.count(50);
    let title = s.title("stall", n);
    let mut rng = Rng::new(s.opts.seed);
    let profile = s.profile();
    let mut run = Run::begin(s, &profile, Duration::ZERO)?;
    run.start(s)?;
    let mut frozen = 0;
    for _ in 0..n {
        frozen += u32::from(edge(&mut run, s, &mut rng, BACKGROUND)?);
        // Half the gaps are a little shorter than Gone after. A freeze at either end then makes
        // the real gap long enough to call for a click, while the engine watched too little of
        // it to re-arm: it must report a possible miss instead of clicking.
        sleep(if rng.coin() { rng.ms(100, 300) } else { rng.ms(30, 38) });
        frozen += u32::from(edge(&mut run, s, &mut rng, GREEN)?);
        sleep(rng.ms(100, 300));
    }
    let mut r = run.finish(s, title, false)?;
    r.notes
        .push(format!("the engine was frozen {frozen} times, 30 ms around an edge"));
    Ok(r)
}

fn two_engines(s: &mut Session) -> Result<Report, LabError> {
    let mut r = Report::named(s.title("two-engines", 1));
    r.sandbox = !s.host.is_real();
    match s.host.second_engine()? {
        Some(3) => r.notes.push("a second glintd exited with code 3".into()),
        Some(code) => r
            .problems
            .push(format!("a second glintd exited with code {code}, not 3")),
        None => r.problems.push("a second glintd kept running".into()),
    }
    match s.try_call(Request::Ping) {
        Ok(Event::Pong) => r.notes.push("the first engine still answers".into()),
        other => r
            .problems
            .push(format!("the first engine stopped answering: {other:?}")),
    }
    Ok(r)
}

fn feedback(s: &mut Session) -> Result<Report, LabError> {
    let n = s.count(3);
    let title = s.title("feedback", n);
    let mut profile = s.profile();
    // A low limit makes the stop come within a few clicks, however busy the machine is.
    profile.trigger.max_clicks_per_sec = 3;
    let limit = profile.trigger.max_clicks_per_sec;
    let mut run = Run::begin(s, &profile, Duration::ZERO)?;
    run.cfg.expect_stop = Some(StopCause::SafetyStop);
    let mut firsts = Vec::new();
    for round in 0..n {
        run.clear(s)?;
        // A Start keeps the clicks of the last second in the count (the stricter choice), so a
        // round right after a stop would trip the stop at once.
        sleep(Duration::from_millis(if round == 0 { 100 } else { 1100 }));
        run.start(s)?;
        sleep(Duration::from_millis(100));
        run.show(s, GREEN)?;
        let begun = Instant::now();
        let mut first = None;
        // Like a game that answers every click with a new appearance.
        while begun.elapsed() < Duration::from_secs(5) {
            match run.next_input(s, Duration::from_millis(150)) {
                Some(i) if matches!(i.kind, InputKind::ButtonPress(_)) => {
                    first.get_or_insert(i.at);
                }
                Some(_) => continue,
                None if !s.state()?.started => break,
                None => {}
            }
            run.clear(s)?;
            sleep(Duration::from_millis(50));
            run.show(s, GREEN)?;
        }
        firsts.push(first);
        run.started = false; // the engine stopped itself (or the check below fails)
    }
    let mut r = run.finish(s, title, false)?;
    // The safety stop's rule (§5.5), checked against the engine's own click times: no second ever
    // held more than `limit` clicks, and each stop came when one more was due within a second. The
    // engine counts by when it decided, a few microseconds before the press it traced: 5 ms of
    // tolerance at the edge of the second.
    let second = Duration::from_secs(1);
    let edge = Duration::from_millis(5);
    let stops: Vec<Instant> = r
        .self_stops
        .iter()
        .filter(|(_, c)| *c == StopCause::SafetyStop)
        .map(|(t, _)| *t)
        .collect();
    if stops.len() != n as usize {
        r.problems.push(format!("{} safety stops in {n} rounds", stops.len()));
    }
    let fired = &r.fired_at;
    for (k, &f) in fired.iter().enumerate() {
        let within = fired[k..].iter().take_while(|&&g| g - f < second - edge).count();
        if within > limit as usize {
            r.problems.push(format!("{within} clicks within one second"));
            break;
        }
    }
    for (round, (first, &stop)) in firsts.iter().zip(&stops).enumerate() {
        let before = fired.iter().filter(|&&f| f <= stop && stop - f < second + edge).count();
        if before != limit as usize {
            r.problems.push(format!(
                "round {}: the stop came after {before} clicks in the last second, not {limit}",
                round + 1
            ));
        }
        if fired.iter().any(|&f| f > stop && f - stop < second) {
            r.problems.push(format!("round {}: a click after the stop", round + 1));
        }
        if let Some(f) = first {
            r.notes.push(format!(
                "round {}: safety stop {} ms after the first click",
                round + 1,
                stop.saturating_duration_since(*f).as_millis()
            ));
        }
    }
    Ok(r)
}

/// Shows the green until the engine responds, up to `attempts` times, and returns its first
/// input. Right after a Start or a profile change the engine re-arms only once it has watched
/// 40 ms of absence, and on a busy machine blind time can start that over: then it reports a
/// possible miss instead of clicking, and the next showing gets the click.
fn show_until_input(s: &Session, attempts: u32) -> Result<Option<Input>, LabError> {
    for _ in 0..attempts {
        s.canvas.show(GREEN)?;
        if let Some(i) = s.canvas.next_input(Duration::from_millis(400)) {
            return Ok(Some(i));
        }
        s.canvas.clear()?;
        sleep(Duration::from_millis(150));
        // A response that came late still counts, rather than going unnoticed.
        if let Some(i) = s.canvas.next_input(Duration::ZERO) {
            return Ok(Some(i));
        }
    }
    Ok(None)
}

/// The key named `name` on this keyboard.
fn key_spec(keymap: &Keymap, name: &str) -> Result<KeySpec, LabError> {
    keysym_from_name(name)
        .and_then(|sym| keymap.keycode(sym))
        .map(|keycode| KeySpec {
            keycode,
            name: name.into(),
        })
        .ok_or_else(|| LabError::Setup(format!("no key {name} on this keyboard")))
}

fn modifier_keycode(keymap: &Keymap, m: Modifier) -> Result<u8, LabError> {
    let sym = match m {
        Modifier::Shift => keysym::SHIFT_L,
        Modifier::Ctrl => keysym::CONTROL_L,
        Modifier::Alt => keysym::ALT_L,
        Modifier::Super => keysym::SUPER_L,
    };
    keymap
        .keycode(sym)
        .ok_or_else(|| LabError::Setup(format!("no {m:?} key on this keyboard")))
}

fn mouse(button: MouseButton, hold_ms: u32, modifiers: Vec<Modifier>) -> Action {
    Action {
        kind: ActionKind::Mouse,
        button,
        modifiers,
        hold_ms,
        ..Action::default()
    }
}

fn key(key: KeySpec, hold_ms: u32, modifiers: Vec<Modifier>) -> Action {
    Action {
        kind: ActionKind::Key,
        key: Some(key),
        modifiers,
        hold_ms,
        ..Action::default()
    }
}

/// What the canvas should see for one click of `action`: modifiers down, the press, the
/// release, modifiers up in reverse order (§6).
fn expected_inputs(keymap: &Keymap, action: &Action) -> Result<Vec<InputKind>, LabError> {
    let mods = action
        .modifiers
        .iter()
        .map(|&m| modifier_keycode(keymap, m))
        .collect::<Result<Vec<_>, _>>()?;
    let (press, release) = match (action.kind, &action.key) {
        (ActionKind::Key, Some(k)) => (InputKind::KeyPress(k.keycode), InputKind::KeyRelease(k.keycode)),
        _ => {
            let b = action.button.x_button();
            (InputKind::ButtonPress(b), InputKind::ButtonRelease(b))
        }
    };
    let mut out: Vec<InputKind> = mods.iter().map(|&m| InputKind::KeyPress(m)).collect();
    out.extend([press, release]);
    out.extend(mods.iter().rev().map(|&m| InputKind::KeyRelease(m)));
    Ok(out)
}

fn action_matrix(s: &mut Session) -> Result<Report, LabError> {
    let rounds = s.count(1);
    let mut r = Report::named(s.title("action-matrix", rounds));
    r.sandbox = !s.host.is_real();
    let keymap = s.canvas.keymap()?;
    let mut cases = Vec::new();
    for button in [
        MouseButton::Left,
        MouseButton::Middle,
        MouseButton::Right,
        MouseButton::Back,
        MouseButton::Forward,
    ] {
        for hold in [0, 10, 50] {
            cases.push((
                format!("{button:?} button, hold {hold} ms"),
                mouse(button, hold, Vec::new()),
            ));
        }
    }
    for name in ["a", "space", "Return", "F1"] {
        for hold in [0, 10, 50] {
            cases.push((
                format!("key {name}, hold {hold} ms"),
                key(key_spec(&keymap, name)?, hold, Vec::new()),
            ));
        }
    }
    cases.push((
        "Ctrl+a, hold 10 ms".into(),
        key(key_spec(&keymap, "a")?, 10, vec![Modifier::Ctrl]),
    ));
    cases.push((
        "Shift+Alt+left button, hold 10 ms".into(),
        mouse(MouseButton::Left, 10, vec![Modifier::Shift, Modifier::Alt]),
    ));
    let base = s.profile();
    s.canvas.clear()?;
    s.set_profile(&base)?;
    s.start()?;
    let mut holds = Vec::new();
    for _ in 0..rounds {
        for (label, action) in &cases {
            let mut p = base.clone();
            p.action = action.clone();
            s.set_profile(&p)?; // restarts, latched
            sleep(Duration::from_millis(80)); // a watched absence re-arms
            s.canvas.take_inputs();
            let want = expected_inputs(&keymap, action)?;
            let mut got: Vec<Input> = show_until_input(s, 3)?.into_iter().collect();
            let deadline = Instant::now() + Duration::from_millis(800 + u64::from(action.hold_ms));
            while got.len() < want.len() {
                match s.canvas.next_input(deadline.saturating_duration_since(Instant::now())) {
                    Some(i) => got.push(i),
                    None => break,
                }
            }
            sleep(Duration::from_millis(40)); // anything extra
            got.extend(s.canvas.take_inputs());
            s.canvas.clear()?;
            sleep(Duration::from_millis(50));
            r.appearances += 1;
            r.clicks += got
                .iter()
                .filter(|i| Some(&i.kind) == want.get(action.modifiers.len()))
                .count() as u32;
            let kinds: Vec<InputKind> = got.iter().map(|i| i.kind).collect();
            if kinds != want {
                r.problems.push(format!("{label}: expected {want:?}, got {kinds:?}"));
                continue;
            }
            let at = |k: InputKind| got.iter().find(|i| i.kind == k).map(|i| i.server_ms);
            let n = action.modifiers.len();
            if let (Some(down), Some(up)) = (at(want[n]), at(want[n + 1])) {
                holds.push((action.hold_ms, up.wrapping_sub(down)));
            }
        }
    }
    s.stop()?;
    r.extra = r.clicks.saturating_sub(r.appearances);
    r.notes.push(format!("{} cases", cases.len()));
    for hold in [0, 10, 50] {
        let seen: Vec<u32> = holds.iter().filter(|h| h.0 == hold).map(|h| h.1).collect();
        if let (Some(lo), Some(hi)) = (seen.iter().min(), seen.iter().max()) {
            r.notes.push(format!(
                "hold {hold} ms: the server saw {lo}–{hi} ms between press and release"
            ));
        }
    }
    Ok(r)
}

fn kill_mid_press(s: &mut Session) -> Result<Report, LabError> {
    let rounds = s.count(2);
    let mut r = Report::named(s.title("kill-mid-press", rounds));
    r.sandbox = !s.host.is_real();
    let keymap = s.canvas.keymap()?;
    let a = key_spec(&keymap, "a")?;
    let ctrl = modifier_keycode(&keymap, Modifier::Ctrl)?;
    for round in 0..rounds {
        let button = round % 2 == 0;
        let (label, action) = if button {
            ("the left button", mouse(MouseButton::Left, 200, Vec::new()))
        } else {
            ("Ctrl+a", key(a.clone(), 200, vec![Modifier::Ctrl]))
        };
        let mut p = s.profile();
        p.action = action;
        s.set_profile(&p)?;
        // The store writes the profile (300 ms debounce), so the next engine knows its key.
        sleep(Duration::from_millis(400));
        s.canvas.clear()?;
        s.start()?;
        sleep(Duration::from_millis(100));
        s.canvas.take_inputs();
        let press = if button {
            InputKind::ButtonPress(1)
        } else {
            InputKind::KeyPress(a.keycode)
        };
        let mut pressed = show_until_input(s, 3)?.is_some_and(|i| i.kind == press);
        let deadline = Instant::now() + Duration::from_secs(1);
        while !pressed && let Some(i) = s.canvas.next_input(deadline.saturating_duration_since(Instant::now())) {
            pressed = i.kind == press;
        }
        r.appearances += 1;
        if !pressed {
            r.problems.push(format!("{label}: no press"));
            s.stop()?;
            continue;
        }
        r.clicks += 1;
        s.host.kill_engine()?;
        let held = |s: &Session, all: bool| -> Result<bool, LabError> {
            if button {
                return Ok(s.canvas.pointer_state()? & BUTTON1_MASK != 0);
            }
            let (k, c) = (s.canvas.key_down(a.keycode)?, s.canvas.key_down(ctrl)?);
            Ok(if all { k && c } else { k || c })
        };
        if !held(s, true)? {
            r.problems
                .push(format!("{label}: already released when the engine was killed"));
        }
        s.canvas.clear()?;
        s.host.restart_engine()?;
        s.engine = s.host.client()?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while held(s, false)? && Instant::now() < deadline {
            sleep(Duration::from_millis(10));
        }
        if held(s, false)? {
            r.problems
                .push(format!("{label}: still held after the engine restarted"));
        } else {
            r.notes.push(format!(
                "{label}: held after the kill, released by the restarted engine"
            ));
        }
    }
    Ok(r)
}

fn action_equals_hotkey(s: &mut Session) -> Result<Report, LabError> {
    let mut r = Report::named(s.title("action-equals-hotkey", 1));
    r.sandbox = !s.host.is_real();
    let keymap = s.canvas.keymap()?;
    for hotkey in ["F7", "F8", "F6"] {
        let mut p = s.profile();
        p.action = key(key_spec(&keymap, hotkey)?, 10, Vec::new());
        match s.try_call(Request::SetProfile { profile: Box::new(p) })? {
            Event::Error { message, .. } => r.notes.push(format!("action key {hotkey}: rejected ({message})")),
            other => r.problems.push(format!("action key {hotkey} was accepted: {other:?}")),
        }
    }
    // The other way round: a hotkey on the active profile's action key.
    let mut p = s.profile();
    p.action = key(key_spec(&keymap, "F9")?, 10, Vec::new());
    s.set_profile(&p)?;
    let mut settings = Settings::default();
    settings.hotkeys.start_stop = "F9".into();
    match s.try_call(Request::SetSettings {
        settings: Box::new(settings),
    })? {
        Event::Error { message, .. } => r
            .notes
            .push(format!("start/stop hotkey F9 on an F9 action: rejected ({message})")),
        other => r
            .problems
            .push(format!("start/stop hotkey F9 on an F9 action was accepted: {other:?}")),
    }
    // Nothing changed: the lab profile still clicks.
    let p = s.profile();
    s.set_profile(&p)?;
    s.canvas.clear()?;
    s.start()?;
    sleep(Duration::from_millis(100));
    s.canvas.take_inputs();
    r.appearances = 1;
    if show_until_input(s, 3)?.is_some_and(|i| i.kind == InputKind::ButtonPress(1)) {
        r.clicks = 1;
    }
    if r.clicks == 0 {
        r.problems.push("no click after the rejected changes".into());
    }
    s.stop()?;
    Ok(r)
}

fn soak(s: &mut Session) -> Result<Report, LabError> {
    let total = s.opts.duration;
    let title = format!("soak --minutes {:.1} · {}", total.as_secs_f64() / 60.0, s.tags());
    let mut rng = Rng::new(s.opts.seed);
    let green = s.profile();
    let blue = blue_profile(&green, ShadePreset::Normal);
    let blue_wider = blue_profile(&green, ShadePreset::Loose);
    let rules = CompiledRules::new(&green.color);
    let pool = tight_shades(&rules, &mut rng, 32);
    let wide = wide_only(&rules).ok_or_else(|| LabError::Setup("no color is only in the wide range".into()))?;
    let shapes = distractor_shapes(s, &green)?;
    // Without near-miss's exclusion, its last color is in the range: leave it out.
    let near = &near_miss_colors()[..3];
    let mut run = Run::begin(s, &green, Duration::from_secs(1))?;
    run.start(s)?;
    let memory_before = s.host.engine_rss_kib();
    let begun = Instant::now();
    let mut steps = 0u32;
    while begun.elapsed() < total {
        steps += 1;
        match rng.below(9) {
            0 | 1 => appear(&mut run, s, &mut rng, GREEN, (60, 600), (30, 500))?,
            2 if !pool.is_empty() => {
                let c = *rng.pick(&pool);
                appear(&mut run, s, &mut rng, c, (60, 400), (30, 300))?;
            }
            3 => flicker_once(&mut run, s, &mut rng)?,
            4 => boundary_once(&mut run, s, &mut rng, wide)?,
            5 => fade_once(&mut run, s, &mut rng)?,
            6 => rapid_round(&mut run, s, &mut rng)?,
            7 => {
                run.clear(s)?;
                sleep(rng.ms(60, 200));
                if rng.coin() {
                    run.show(s, *rng.pick(near))?;
                } else {
                    run.beside(s, GREEN, rng.pick(&shapes))?;
                }
                sleep(rng.ms(50, 300));
            }
            _ => {
                run.clear(s)?;
                sleep(rng.ms(60, 150));
                run.edit(s, &blue)?;
                appear(&mut run, s, &mut rng, BLUE, (60, 200), (50, 200))?;
                run.edit(s, &blue_wider)?;
                sleep(rng.ms(50, 150));
                run.clear(s)?;
                sleep(rng.ms(60, 150));
                run.edit(s, &green)?;
            }
        }
    }
    let memory_after = s.host.engine_rss_kib();
    let mut r = run.finish(s, title, false)?;
    r.notes.push(format!("{steps} randomized steps"));
    if let (Some(before), Some(after)) = (memory_before, memory_after) {
        r.notes.push(format!(
            "engine memory {:.1} MiB → {:.1} MiB",
            before as f64 / 1024.0,
            after as f64 / 1024.0
        ));
        if after > before + before / 2 && after - before > 8 * 1024 {
            r.problems.push("the engine's memory grew by more than half".into());
        }
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use glint_core::color::Lut;

    use super::*;

    #[test]
    fn the_near_miss_colors_never_count() {
        let mut profile = lab_profile();
        profile.color.exclude = vec![near_miss_exclusion()];
        let lut = Lut::build(&CompiledRules::new(&profile.color));
        let classes: Vec<Class> = near_miss_colors().iter().map(|&c| lut.class(c)).collect();
        assert_eq!(classes, [Class::Absent, Class::Absent, Class::Absent, Class::Excluded]);
        assert_eq!(lut.class(GREEN), Class::Tight);
    }

    #[test]
    fn the_colors_the_scenarios_need_exist() {
        let rules = CompiledRules::new(&lab_profile().color);
        let wide = wide_only(&rules).unwrap();
        assert_eq!(rules.classify(Rgb::from_u32(wide)), Class::Wide);
        let pool = tight_shades(&rules, &mut Rng::new(7), 64);
        assert_eq!(pool.len(), 64);
        // The fade passes through the wide range on its way to the tight one.
        let classes: Vec<Class> = (0..=20)
            .map(|k| rules.classify(Rgb::from_u32(mix(BACKGROUND, GREEN, k, 20))))
            .collect();
        assert_eq!(classes.first(), Some(&Class::Absent));
        assert_eq!(classes.last(), Some(&Class::Tight));
        // The blue is outside the green's range and the other way round.
        let blue = CompiledRules::new(&blue_profile(&lab_profile(), ShadePreset::Normal).color);
        assert_eq!(rules.classify(Rgb::from_u32(BLUE)), Class::Absent);
        assert_eq!(blue.classify(Rgb::from_u32(GREEN)), Class::Absent);
        assert_eq!(blue.classify(Rgb::from_u32(BLUE)), Class::Tight);
    }

    #[test]
    fn mixing() {
        assert_eq!(mix(0x000000, 0xFF_80_40, 1, 2), 0x7F_40_20);
        assert_eq!(mix(GREEN, BACKGROUND, 0, 9), GREEN);
        assert_eq!(mix(GREEN, BACKGROUND, 9, 9), BACKGROUND);
    }
}
