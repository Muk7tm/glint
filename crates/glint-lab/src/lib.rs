// SPDX-License-Identifier: GPL-3.0-or-later
//! The Glint test lab (PLAN §16).
//!
//! A [`Canvas`] window covers the monitor, shows colors in a patch at its center, and records the
//! clicks the engine sends into it. Each scenario drives a running engine through its socket and
//! checks one click per appearance against what the canvas actually showed.
//!
//! Response time runs from just before the drawing request is sent to the moment the engine's
//! `ButtonPress` arrives on the canvas. The lab still follows each drawing request with a round
//! trip, so it knows the server has drawn the color, but doesn't timestamp that moment: the engine
//! answers in a few hundred microseconds, and its click sometimes reached the lab's event thread
//! before the round trip's reply woke the drawing thread. Timing from the request can't put a
//! click before its appearance; it overstates the response by the request's one-way trip to the
//! server, about 10 µs.
//!
//! Phase 2 has `basic`, `near-miss` and `arm-while-present`; Phase 3 adds the rest of §16.
#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use glint_core::color::{ColorEntry, ColorModel, ShadePreset};
use glint_core::pattern::Rect;
use glint_core::{Class, Profile, Rgb};
use glint_ipc::{Client, Event, Request};
use glint_x11::env::LocalDisplay;
use x11rb::connection::Connection;
use x11rb::protocol::Event as XEvent;
use x11rb::protocol::xproto::{
    ChangeGCAux, ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask, InputFocus, Rectangle, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// The lab's target color: Appendix B's green.
pub const GREEN: u32 = 0x3B_D1_6F;
/// The canvas's background.
pub const BACKGROUND: u32 = 0x10_10_10;
/// The color patch: larger than any built-in pattern's rectangle.
const PATCH: u16 = 40;
const WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum LabError {
    #[error("X: {0}")]
    X(String),
    #[error("engine: {0}")]
    Engine(String),
}

fn x<E: std::fmt::Display>(e: E) -> LabError {
    LabError::X(e.to_string())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Click {
    Press(u8),
    Release(u8),
}

/// A window covering a monitor, with a color patch at the monitor's center.
pub struct Canvas {
    conn: Arc<RustConnection>,
    win: u32,
    gc: u32,
    patch: Rectangle,
    clicks: Receiver<(Instant, Click)>,
}

impl Canvas {
    /// Opens the canvas over `area` (in root coordinates) and parks the pointer inside it, away
    /// from the patch, so the engine's clicks land on the canvas.
    pub fn open(display: LocalDisplay, area: Rect) -> Result<Self, LabError> {
        let conn = Arc::new(glint_x11::conn::open(display).map_err(x)?);
        let root = conn.setup().roots[usize::from(display.screen)].root;
        let win = conn.generate_id().map_err(x)?;
        let aux = CreateWindowAux::new()
            .background_pixel(BACKGROUND)
            .event_mask(EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE)
            .override_redirect(1);
        let (ax, ay) = (i16::try_from(area.x).map_err(x)?, i16::try_from(area.y).map_err(x)?);
        conn.create_window(
            0,
            win,
            root,
            ax,
            ay,
            area.w,
            area.h,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &aux,
        )
        .map_err(x)?;
        conn.map_window(win).map_err(x)?;
        conn.warp_pointer(x11rb::NONE, win, 0, 0, 0, 0, 20, 20).map_err(x)?;
        conn.set_input_focus(InputFocus::PARENT, win, x11rb::CURRENT_TIME)
            .map_err(x)?;
        let gc = conn.generate_id().map_err(x)?;
        conn.create_gc(gc, win, &CreateGCAux::new().foreground(BACKGROUND))
            .map_err(x)?;
        conn.sync().map_err(x)?;
        // The patch around the monitor's anchor pixel (§5.1), in window coordinates.
        let (cx, cy) = area.anchor();
        let half = (PATCH / 2) as i32;
        let patch = Rectangle {
            x: i16::try_from(cx - area.x - half).map_err(x)?,
            y: i16::try_from(cy - area.y - half).map_err(x)?,
            width: PATCH,
            height: PATCH,
        };
        let (tx, clicks) = mpsc::channel();
        let reader = Arc::clone(&conn);
        std::thread::Builder::new()
            .name("lab-events".into())
            .spawn(move || {
                while let Ok(event) = reader.wait_for_event() {
                    let at = Instant::now();
                    let click = match event {
                        XEvent::ButtonPress(e) if e.event == win => Click::Press(e.detail),
                        XEvent::ButtonRelease(e) if e.event == win => Click::Release(e.detail),
                        _ => continue,
                    };
                    if tx.send((at, click)).is_err() {
                        break;
                    }
                }
            })
            .map_err(x)?;
        Ok(Self {
            conn,
            win,
            gc,
            patch,
            clicks,
        })
    }

    /// Fills the patch with `color`, waits until the server has drawn it, and returns the moment
    /// just before the drawing request went out.
    pub fn show(&self, color: u32) -> Result<Instant, LabError> {
        self.conn
            .change_gc(self.gc, &ChangeGCAux::new().foreground(color))
            .map_err(x)?;
        let sent = Instant::now();
        self.conn
            .poly_fill_rectangle(self.win, self.gc, &[self.patch])
            .map_err(x)?;
        self.conn.sync().map_err(x)?;
        Ok(sent)
    }

    pub fn clear(&self) -> Result<Instant, LabError> {
        self.show(BACKGROUND)
    }

    /// Every click received since the last call.
    pub fn take_clicks(&self) -> Vec<(Instant, Click)> {
        self.clicks.try_iter().collect()
    }
}

/// What a scenario measured.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Report {
    pub scenario: String,
    pub appearances: u32,
    pub clicks: u32,
    pub extra: u32,
    pub missed: u32,
    /// Response times of the appearances that were clicked.
    pub latencies: Vec<Duration>,
}

impl Report {
    pub fn passed(&self) -> bool {
        self.extra == 0 && self.missed == 0
    }

    /// The §16 report.
    pub fn summary(&self) -> String {
        let mut lat = self.latencies.clone();
        lat.sort_unstable();
        let at = |q: f64| {
            lat.get(((lat.len() as f64 * q) as usize).min(lat.len().saturating_sub(1)))
                .copied()
        };
        let ms = |d: Option<Duration>| d.map_or_else(|| "-".into(), |d| format!("{:.2}", d.as_secs_f64() * 1e3));
        format!(
            "glint-lab {}\nappearances {} · clicks {} · extra {} · missed {}\nresponse    p50 {} ms · p95 {} ms · p99 {} ms · max {} ms\nverdict     {}\n",
            self.scenario,
            self.appearances,
            self.clicks,
            self.extra,
            self.missed,
            ms(at(0.5)),
            ms(at(0.95)),
            ms(at(0.99)),
            ms(lat.last().copied()),
            if self.passed() { "PASS" } else { "FAIL" },
        )
    }
}

/// A small deterministic generator for the scenarios' timings.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    /// A duration between `lo` and `hi` milliseconds.
    pub fn ms(&mut self, lo: u64, hi: u64) -> Duration {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        Duration::from_millis(lo + self.0 % (hi - lo + 1))
    }
}

fn call(engine: &mut Client, request: Request) -> Result<Event, LabError> {
    match engine
        .call(request, WAIT)
        .map_err(|e| LabError::Engine(e.to_string()))?
    {
        Event::Error { message, .. } => Err(LabError::Engine(message)),
        e => Ok(e),
    }
}

/// The lab's profile: the green, Normal shades, everything else default.
pub fn lab_profile() -> Profile {
    let mut p = Profile::new("glint-lab");
    p.color.include = vec![ShadePreset::Normal.entry(ColorModel::Oklch, Rgb::from_u32(GREEN))];
    p
}

/// Counts the presses inside each appearance's window: one is right, none is a miss, more are
/// extra. Presses before the first appearance are extra too.
fn score(report: &mut Report, starts: &[Instant], end: Instant, clicks: &[(Instant, Click)]) {
    let presses: Vec<Instant> = clicks
        .iter()
        .filter(|(_, c)| matches!(c, Click::Press(_)))
        .map(|(t, _)| *t)
        .collect();
    report.clicks = presses.len() as u32;
    report.extra += presses
        .iter()
        .filter(|&&t| starts.first().is_none_or(|&s| t < s))
        .count() as u32;
    for (i, &start) in starts.iter().enumerate() {
        let until = starts.get(i + 1).copied().unwrap_or(end);
        let inside: Vec<Instant> = presses.iter().copied().filter(|&t| t >= start && t < until).collect();
        match inside.first() {
            None => report.missed += 1,
            Some(&first) => {
                report.latencies.push(first - start);
                report.extra += inside.len() as u32 - 1;
            }
        }
    }
}

/// `basic` (§16): `n` appearances of the exact color, on for 30–500 ms and off for 60–600 ms at
/// random. Pass: exactly `n` clicks, one per appearance.
pub fn basic(canvas: &Canvas, engine: &mut Client, n: u32, seed: u64) -> Result<Report, LabError> {
    let mut rng = Rng::new(seed);
    call(
        engine,
        Request::SetProfile {
            profile: Box::new(lab_profile()),
        },
    )?;
    canvas.clear()?;
    call(engine, Request::Start)?;
    canvas.take_clicks();
    let mut starts = Vec::new();
    for _ in 0..n {
        canvas.clear()?;
        std::thread::sleep(rng.ms(60, 600));
        starts.push(canvas.show(GREEN)?);
        std::thread::sleep(rng.ms(30, 500));
    }
    canvas.clear()?;
    std::thread::sleep(Duration::from_millis(200));
    let end = Instant::now();
    call(engine, Request::Stop)?;
    let mut report = Report {
        scenario: format!("basic --count {n}"),
        appearances: n,
        ..Report::default()
    };
    score(&mut report, &starts, end, &canvas.take_clicks());
    Ok(report)
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

/// `near-miss` (§16): colors just outside the wide range, plus the exclude colors. Pass: no clicks.
pub fn near_miss(canvas: &Canvas, engine: &mut Client, n: u32, seed: u64) -> Result<Report, LabError> {
    let mut rng = Rng::new(seed);
    let mut profile = lab_profile();
    let mut exclusion = ColorEntry::new(Rgb::from_u32(0x40_D5_75));
    exclusion.tol_l = Some(0.01);
    exclusion.tol_c = Some(0.01);
    exclusion.tol_h = Some(1.0);
    profile.color.exclude = vec![exclusion];
    call(
        engine,
        Request::SetProfile {
            profile: Box::new(profile),
        },
    )?;
    // The engine's own table must agree that none of these count.
    let colors = near_miss_colors();
    if let Event::Classified { verdicts } = call(
        engine,
        Request::Classify {
            colors: colors.iter().map(|&c| Rgb::from_u32(c)).collect(),
        },
    )? && verdicts.iter().any(|v| matches!(v, Class::Tight | Class::Wide))
    {
        return Err(LabError::Engine(format!("a near-miss color counts: {verdicts:?}")));
    }
    canvas.clear()?;
    call(engine, Request::Start)?;
    canvas.take_clicks();
    for i in 0..n {
        canvas.clear()?;
        std::thread::sleep(rng.ms(60, 200));
        canvas.show(colors[i as usize % colors.len()])?;
        std::thread::sleep(rng.ms(50, 300));
    }
    canvas.clear()?;
    std::thread::sleep(Duration::from_millis(200));
    call(engine, Request::Stop)?;
    let clicks = canvas.take_clicks();
    let presses = clicks.iter().filter(|(_, c)| matches!(c, Click::Press(_))).count() as u32;
    Ok(Report {
        scenario: format!("near-miss --count {n}"),
        appearances: 0,
        clicks: presses,
        extra: presses,
        ..Report::default()
    })
}

/// `arm-while-present` (§16): Start while the color is showing. Pass: no click until it leaves
/// and returns, then exactly one.
pub fn arm_while_present(canvas: &Canvas, engine: &mut Client, n: u32) -> Result<Report, LabError> {
    call(
        engine,
        Request::SetProfile {
            profile: Box::new(lab_profile()),
        },
    )?;
    let mut report = Report {
        scenario: format!("arm-while-present --count {n}"),
        ..Report::default()
    };
    for _ in 0..n {
        canvas.show(GREEN)?;
        canvas.take_clicks();
        call(engine, Request::Start)?;
        std::thread::sleep(Duration::from_millis(250));
        let while_present = canvas
            .take_clicks()
            .iter()
            .filter(|(_, c)| matches!(c, Click::Press(_)))
            .count() as u32;
        report.extra += while_present;
        canvas.clear()?;
        std::thread::sleep(Duration::from_millis(100));
        let start = canvas.show(GREEN)?;
        std::thread::sleep(Duration::from_millis(150));
        let end = Instant::now();
        let clicks = canvas.take_clicks();
        let mut one = Report::default();
        score(&mut one, &[start], end, &clicks);
        report.appearances += 1;
        report.clicks += one.clicks + while_present;
        report.extra += one.extra;
        report.missed += one.missed;
        report.latencies.extend(one.latencies);
        call(engine, Request::Stop)?;
        canvas.clear()?;
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(report)
}

/// A throwaway Xvfb with a real `glintd` on it, its runtime and config directories private to it.
/// Nothing here touches the real desktop.
pub struct Sandbox {
    xvfb: glint_x11::sandbox::Xvfb,
    engine: std::process::Child,
    scratch: std::path::PathBuf,
    socket: std::path::PathBuf,
}

impl Sandbox {
    /// `Ok(None)` when Xvfb isn't installed.
    pub fn start(glintd: &std::path::Path, width: u16, height: u16) -> Result<Option<Self>, LabError> {
        use glint_x11::sandbox::{SandboxError, Xvfb};
        let xvfb = match Xvfb::start(width, height) {
            Ok(x) => x,
            Err(SandboxError::NotInstalled) => return Ok(None),
            Err(e) => return Err(LabError::X(e.to_string())),
        };
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let scratch = std::env::temp_dir().join(format!("glint-lab-{}-{n}", std::process::id()));
        std::fs::create_dir_all(scratch.join("run")).map_err(x)?;
        let engine = std::process::Command::new(glintd)
            .env("DISPLAY", xvfb.display().to_string())
            .env("XDG_RUNTIME_DIR", scratch.join("run"))
            .env("XDG_CONFIG_HOME", scratch.join("config"))
            .env("XDG_STATE_HOME", scratch.join("state"))
            .stdin(std::process::Stdio::null())
            .spawn()
            .map_err(|e| LabError::Engine(format!("{}: {e}", glintd.display())))?;
        let socket = scratch
            .join("run/glint")
            .join(format!("glintd-{}.sock", xvfb.display().number));
        let deadline = Instant::now() + WAIT;
        while !socket.exists() {
            if Instant::now() > deadline {
                return Err(LabError::Engine("glintd didn't start".into()));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(Some(Self {
            xvfb,
            engine,
            scratch,
            socket,
        }))
    }

    pub fn display(&self) -> LocalDisplay {
        self.xvfb.display()
    }

    /// A client that has said hello.
    pub fn client(&self) -> Result<Client, LabError> {
        let mut c = Client::connect(&self.socket).map_err(|e| LabError::Engine(e.to_string()))?;
        c.hello(glint_ipc::ClientKind::Lab, WAIT)
            .map_err(|e| LabError::Engine(e.to_string()))?;
        Ok(c)
    }

    /// The whole screen, which is the sandbox's only monitor.
    pub fn screen(&self) -> Result<Rect, LabError> {
        let conn = glint_x11::conn::open(self.display()).map_err(x)?;
        let s = &conn.setup().roots[0];
        Ok(Rect::new(0, 0, s.width_in_pixels, s.height_in_pixels))
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = self.engine.kill();
        let _ = self.engine.wait();
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

/// An engine on the real desktop for `glint-lab --real-desktop`: a fresh `glintd` on `$DISPLAY`
/// with a throwaway config directory (your profiles and settings stay untouched) but the real
/// runtime directory, so the single-instance lock refuses it if an engine already runs there. It
/// exits with the lab, and the lab connects as a UI, so if the lab dies the engine stops watching
/// at once.
pub struct RealDesktop {
    engine: std::process::Child,
    scratch: std::path::PathBuf,
    socket: std::path::PathBuf,
    display: LocalDisplay,
}

impl RealDesktop {
    pub fn start(glintd: &std::path::Path) -> Result<Self, LabError> {
        let display = LocalDisplay::from_env().map_err(x)?;
        let scratch = std::env::temp_dir().join(format!("glint-lab-real-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).map_err(x)?;
        let mut engine = std::process::Command::new(glintd)
            .arg("--exit-with-parent")
            .env("XDG_CONFIG_HOME", scratch.join("config"))
            .env("XDG_STATE_HOME", scratch.join("state"))
            .stdin(std::process::Stdio::null())
            .spawn()
            .map_err(|e| LabError::Engine(format!("{}: {e}", glintd.display())))?;
        let socket = glint_ipc::socket_path(display.number);
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = engine.try_wait().map_err(x)? {
                let _ = std::fs::remove_dir_all(&scratch);
                return Err(LabError::Engine(if status.code() == Some(3) {
                    "an engine is already running on this display; quit it first (glint-ctl quit)".into()
                } else {
                    format!("glintd exited: {status}")
                }));
            }
            if Client::connect(&socket).is_ok() {
                break;
            }
            if Instant::now() > deadline {
                return Err(LabError::Engine("glintd didn't start".into()));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(Self {
            engine,
            scratch,
            socket,
            display,
        })
    }

    /// A client that identifies as a UI, so its disappearance stops watching.
    pub fn client(&self) -> Result<Client, LabError> {
        let mut c = Client::connect(&self.socket).map_err(|e| LabError::Engine(e.to_string()))?;
        c.hello(glint_ipc::ClientKind::Ui, WAIT)
            .map_err(|e| LabError::Engine(e.to_string()))?;
        Ok(c)
    }

    /// The monitor the lab profile watches (`"primary"`, or the first when none is).
    pub fn monitor(&self) -> Result<Rect, LabError> {
        let conn = glint_x11::conn::open(self.display).map_err(x)?;
        let root = conn.setup().roots[usize::from(self.display.screen)].root;
        let size = (
            conn.setup().roots[0].width_in_pixels,
            conn.setup().roots[0].height_in_pixels,
        );
        let caps = glint_x11::caps::probe(&conn).map_err(x)?;
        let list = glint_x11::monitors::list(&conn, root, size, caps.randr).map_err(x)?;
        glint_x11::monitors::pick(&list, "primary")
            .map(|m| m.rect)
            .ok_or_else(|| LabError::X("no monitor".into()))
    }
}

impl Drop for RealDesktop {
    fn drop(&mut self) {
        if let Ok(mut c) = Client::connect(&self.socket) {
            let _ = c.call(Request::Quit, WAIT);
        }
        let deadline = Instant::now() + WAIT;
        while self.engine.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.engine.kill();
        let _ = self.engine.wait();
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

/// Runs one scenario on the real desktop (see [`RealDesktop`]): a full-monitor canvas on top of
/// everything takes the engine's clicks.
pub fn run_on_real_desktop(
    glintd: &std::path::Path,
    scenario: &str,
    count: u32,
    seed: u64,
) -> Result<Report, LabError> {
    let desktop = RealDesktop::start(glintd)?;
    let canvas = Canvas::open(desktop.display, desktop.monitor()?)?;
    let mut engine = desktop.client()?;
    match scenario {
        "basic" => basic(&canvas, &mut engine, count, seed),
        "near-miss" => near_miss(&canvas, &mut engine, count, seed),
        "arm-while-present" => arm_while_present(&canvas, &mut engine, count),
        other => Err(LabError::Engine(format!(
            "unknown scenario {other:?}; Phase 2 has {}",
            SCENARIOS.join(", ")
        ))),
    }
}

/// The scenarios Phase 2 has.
pub const SCENARIOS: [&str; 3] = ["basic", "near-miss", "arm-while-present"];

/// Runs one scenario in a fresh sandbox. `Ok(None)` when Xvfb isn't installed.
pub fn run_in_sandbox(
    glintd: &std::path::Path,
    scenario: &str,
    count: u32,
    seed: u64,
) -> Result<Option<Report>, LabError> {
    let Some(sandbox) = Sandbox::start(glintd, 1280, 720)? else {
        return Ok(None);
    };
    let canvas = Canvas::open(sandbox.display(), sandbox.screen()?)?;
    let mut engine = sandbox.client()?;
    let report = match scenario {
        "basic" => basic(&canvas, &mut engine, count, seed)?,
        "near-miss" => near_miss(&canvas, &mut engine, count, seed)?,
        "arm-while-present" => arm_while_present(&canvas, &mut engine, count)?,
        other => {
            return Err(LabError::Engine(format!(
                "unknown scenario {other:?}; Phase 2 has {}",
                SCENARIOS.join(", ")
            )));
        }
    };
    Ok(Some(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoring() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let starts = [ms(100), ms(300), ms(500)];
        let clicks = [
            (ms(50), Click::Press(1)),  // before anything showed: extra
            (ms(101), Click::Press(1)), // appearance 1
            (ms(111), Click::Release(1)),
            (ms(102), Click::Press(1)), // appearance 1 again: extra
            (ms(502), Click::Press(1)), // appearance 3 (2 was missed)
        ];
        let mut r = Report::default();
        score(&mut r, &starts, ms(700), &clicks);
        assert_eq!((r.clicks, r.extra, r.missed), (4, 2, 1));
        assert_eq!(r.latencies, [Duration::from_millis(1), Duration::from_millis(2)]);
        assert!(!r.passed());
        assert!(r.summary().contains("extra 2 · missed 1"));
    }

    #[test]
    fn the_near_miss_colors_never_count() {
        use glint_core::color::{CompiledRules, Lut};
        let mut profile = lab_profile();
        let mut exclusion = ColorEntry::new(Rgb::from_u32(0x40_D5_75));
        exclusion.tol_l = Some(0.01);
        exclusion.tol_c = Some(0.01);
        exclusion.tol_h = Some(1.0);
        profile.color.exclude = vec![exclusion];
        let lut = Lut::build(&CompiledRules::new(&profile.color));
        let classes: Vec<Class> = near_miss_colors().iter().map(|&c| lut.class(c)).collect();
        assert_eq!(classes, [Class::Absent, Class::Absent, Class::Absent, Class::Excluded]);
        assert_eq!(lut.class(GREEN), Class::Tight);
    }
}
