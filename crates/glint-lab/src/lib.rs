// SPDX-License-Identifier: MIT
//! The Glint test lab (PLAN §16).
//!
//! A [`Canvas`] window covers the monitor and shows colors at the watched spot, and each
//! scenario drives a real `glintd` through its socket. The lab records:
//!
//! - what it drew, with the two moments that bound when the server drew it (the request going
//!   out, and its round trip coming back);
//! - the input the engine sent into the canvas;
//! - the engine's trace stream: every click with its timing, possible misses, blind time and
//!   self-stops;
//! - a probe client's round trips, which show the X server's own stalls.
//!
//! [`judge`] then replays the drawing against the trigger's rules and decides, appearance by
//! appearance, whether a click was required, allowed, or wrong.
//!
//! Response time runs from just before an appearance's drawing request to the moment the
//! engine's press arrives on the canvas. Timing from the request can't put a click before its
//! appearance; it overstates the response by the request's one-way trip to the server, about
//! 10 µs (Phase 2).
#![forbid(unsafe_code)]

mod canvas;
mod host;
mod judge;
mod probe;
mod scenarios;
mod trace;

use std::path::Path;
use std::time::{Duration, Instant};

pub use canvas::{BACKGROUND, Canvas, Drawn, GREEN, Input, InputKind};
use glint_core::Profile;
use glint_core::pattern::PatternKind;
use glint_core::profile::{Backend, Speed};
use glint_ipc::{Client, Event, Request, StateSnapshot};
pub use host::{Host, RealDesktop, Sandbox};
pub use judge::{Change, Evidence, JudgeCfg, Mark, Report, Shown, Split, judge};
pub use probe::{Probe, ProbeResult};
pub use scenarios::{LATER, Rng, SCENARIOS, Scenario, lab_profile, near_miss_colors};
pub use trace::TraceTap;

/// How long the lab waits for the engine.
pub(crate) const WAIT: Duration = Duration::from_secs(5);
/// How long a new engine may take to listen. It builds its color table first (16,777,216 entries), which on
/// a busy machine, or at idle priority, can take far longer than usual; no scenario times the start.
pub(crate) const START_WAIT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum LabError {
    #[error("X: {0}")]
    X(String),
    #[error("engine: {0}")]
    Engine(String),
    #[error("{0}")]
    Setup(String),
}

pub(crate) fn x<E: std::fmt::Display>(e: E) -> LabError {
    LabError::X(e.to_string())
}

pub(crate) fn engine_err<E: std::fmt::Display>(e: E) -> LabError {
    LabError::Engine(e.to_string())
}

/// What a run is asked to do.
#[derive(Clone, Debug)]
pub struct Options {
    /// Appearances, flashes or rounds; `None` takes the scenario's default.
    pub count: Option<u32>,
    pub seed: u64,
    pub speed: Speed,
    /// How the engine reads the screen (§5.2); window mode can read the window's own image.
    pub backend: Backend,
    /// The sample pattern; `None` keeps the lab profile's 3×3 (ADR 0025 adds `Area`).
    pub pattern: Option<PatternKind>,
    /// How long `soak` runs.
    pub duration: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            count: None,
            seed: 1,
            speed: Speed::Balanced,
            backend: Backend::Auto,
            pattern: None,
            duration: Duration::from_secs(3600),
        }
    }
}

/// A canvas and a connection to the engine on one host, for one scenario.
pub struct Session<'h> {
    pub host: &'h mut dyn Host,
    pub canvas: Canvas,
    pub engine: Client,
    pub opts: Options,
    backend: String,
}

impl<'h> Session<'h> {
    pub fn new(host: &'h mut dyn Host, opts: Options) -> Result<Self, LabError> {
        let canvas = Canvas::open(host.display(), host.monitor()?)?;
        let engine = host.client()?;
        let mut s = Self {
            host,
            canvas,
            engine,
            opts,
            backend: String::new(),
        };
        // The lab's profile first, so the backend named in report titles is the run's.
        let profile = s.profile();
        s.set_profile(&profile)?;
        s.backend = s.state()?.backend;
        Ok(s)
    }

    /// The count asked for, or the scenario's default.
    pub fn count(&self, default: u32) -> u32 {
        self.opts.count.unwrap_or(default).max(1)
    }

    /// The run's settings for a report title.
    pub fn tags(&self) -> String {
        let mut tags = format!("speed {} · backend {}", speed_name(self.opts.speed), self.backend);
        if let Some(pattern) = self.opts.pattern {
            tags.push_str(&format!(" · pattern {}", pattern_name(pattern)));
        }
        tags
    }

    /// A report title in the §16 form.
    pub fn title(&self, scenario: &str, count: u32) -> String {
        format!("{scenario} --count {count} · {}", self.tags())
    }

    /// The lab's profile at the run's speed.
    pub fn profile(&self) -> Profile {
        let mut p = lab_profile();
        p.sampling.speed = self.opts.speed;
        p.sampling.backend = self.opts.backend;
        if let Some(pattern) = self.opts.pattern {
            // A new pattern takes its own Must match, as in the window.
            p.sampling.pattern = pattern;
            p.sampling.k_enter = None;
            p.sampling.k_stay = None;
        }
        p
    }

    /// Reads which backend the engine chose for the profile now in use, for the report title.
    pub fn refresh_backend(&mut self) -> Result<(), LabError> {
        self.backend = self.state()?.backend;
        Ok(())
    }

    /// A request whose error reply is a failure.
    pub fn call(&mut self, request: Request) -> Result<Event, LabError> {
        match self.try_call(request)? {
            Event::Error { message, field, .. } => Err(LabError::Engine(match field {
                Some(f) => format!("{f}: {message}"),
                None => message,
            })),
            e => Ok(e),
        }
    }

    /// A request whose error reply is an answer.
    pub fn try_call(&mut self, request: Request) -> Result<Event, LabError> {
        self.engine.call(request, WAIT).map_err(engine_err)
    }

    pub fn set_profile(&mut self, profile: &Profile) -> Result<(), LabError> {
        self.call(Request::SetProfile {
            profile: Box::new(profile.clone()),
        })
        .map(drop)
    }

    /// Starts watching; returns when the request went out and when its reply came back.
    pub fn start(&mut self) -> Result<Drawn, LabError> {
        self.timed(Request::Start)
    }

    pub fn stop(&mut self) -> Result<Drawn, LabError> {
        self.timed(Request::Stop)
    }

    fn timed(&mut self, request: Request) -> Result<Drawn, LabError> {
        let sent = Instant::now();
        self.call(request)?;
        Ok(Drawn {
            sent,
            drawn: Instant::now(),
        })
    }

    pub fn state(&mut self) -> Result<StateSnapshot, LabError> {
        match self.call(Request::GetState)? {
            Event::State(s) => Ok(s),
            e => Err(LabError::Engine(format!("unexpected reply {e:?}"))),
        }
    }
}

pub fn speed_name(speed: Speed) -> String {
    match speed {
        Speed::Eco => "eco".into(),
        Speed::Balanced => "balanced".into(),
        Speed::Max => "max".into(),
        Speed::PerSecond(n) => format!("{n}/s"),
    }
}

/// `eco`, `balanced`, `max`, or a number of checks per second.
pub fn parse_speed(text: &str) -> Result<Speed, String> {
    match text {
        "eco" => Ok(Speed::Eco),
        "balanced" => Ok(Speed::Balanced),
        "max" => Ok(Speed::Max),
        n => n
            .parse()
            .map(Speed::PerSecond)
            .map_err(|_| format!("{n:?}: use eco, balanced, max, or a number of checks per second")),
    }
}

/// Parses a `--backend` value.
pub fn parse_backend(text: &str) -> Result<Backend, String> {
    match text {
        "auto" => Ok(Backend::Auto),
        "screen" => Ok(Backend::Screen),
        "screen-shm" => Ok(Backend::ScreenShm),
        "window-buffer" => Ok(Backend::WindowBuffer),
        "damage" => Ok(Backend::Damage),
        n => Err(format!("{n:?}: use auto, screen, screen-shm, window-buffer or damage")),
    }
}

/// `--pattern`: the sample patterns the window offers (§5.3, ADR 0025).
pub fn parse_pattern(text: &str) -> Result<PatternKind, String> {
    match text {
        "pixel" => Ok(PatternKind::Pixel),
        "2x2" => Ok(PatternKind::Center2x2),
        "3x3" => Ok(PatternKind::Square3),
        "5x5" => Ok(PatternKind::Square5),
        "plus" => Ok(PatternKind::Plus),
        "area" => Ok(PatternKind::Area),
        n => Err(format!("{n:?}: use pixel, 2x2, 3x3, 5x5, plus or area")),
    }
}

fn pattern_name(p: PatternKind) -> &'static str {
    match p {
        PatternKind::Pixel => "pixel",
        PatternKind::Center2x2 => "2x2",
        PatternKind::Square3 => "3x3",
        PatternKind::Square5 => "5x5",
        PatternKind::Plus => "plus",
        PatternKind::Area => "area",
        PatternKind::Custom => "custom",
    }
}

/// The scenario called `name`.
pub fn scenario(name: &str) -> Result<&'static Scenario, LabError> {
    SCENARIOS.iter().find(|s| s.name == name).ok_or_else(|| {
        LabError::Setup(match LATER.iter().find(|(n, _)| *n == name) {
            Some((n, why)) => format!("{n} {why}"),
            None => format!(
                "unknown scenario {name:?}; there are {}",
                SCENARIOS.iter().map(|s| s.name).collect::<Vec<_>>().join(", ")
            ),
        })
    })
}

/// Runs one scenario in a fresh sandbox: its own Xvfb and its own `glintd`. `Ok(None)` when Xvfb
/// isn't installed.
pub fn run_in_sandbox(glintd: &Path, name: &str, opts: &Options) -> Result<Option<Report>, LabError> {
    let scenario = scenario(name)?;
    let Some(mut sandbox) = Sandbox::start(glintd, 1280, 720)? else {
        return Ok(None);
    };
    let composited = sandbox.composited();
    let mut session = Session::new(&mut sandbox, opts.clone())?;
    let mut report = (scenario.run)(&mut session)?;
    if composited {
        report.notes.push("picom was compositing the sandbox".into());
    }
    Ok(Some(report))
}

/// Runs one scenario on the real desktop (see [`RealDesktop`]).
pub fn run_on_real_desktop(glintd: &Path, name: &str, opts: &Options) -> Result<Report, LabError> {
    let scenario = scenario(name)?;
    if let Some(why) = scenario.sandbox_only {
        return Err(LabError::Setup(format!("{name} runs only in the sandbox: {why}")));
    }
    let mut desktop = RealDesktop::start(glintd)?;
    let mut session = Session::new(&mut desktop, opts.clone())?;
    (scenario.run)(&mut session)
}
