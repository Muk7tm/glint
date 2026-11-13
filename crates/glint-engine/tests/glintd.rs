// SPDX-License-Identifier: GPL-3.0-or-later
//! The real `glintd` on Xvfb, driven only through its socket (§10).
#![allow(clippy::unwrap_used)] // test helpers: any failure should panic

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use glint_core::color::{ColorModel, ShadePreset};
use glint_core::{Class, Profile, Rgb};
use glint_ipc::{Client, ClientKind, Event, Phase, Request};
use glint_x11::conn;
use glint_x11::sandbox::{SandboxError, Xvfb};
use x11rb::connection::Connection;
use x11rb::protocol::Event as XEvent;
use x11rb::protocol::xproto::{ConnectionExt as _, CreateWindowAux, EventMask, InputFocus, WindowClass};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const GREEN: u32 = 0x3B_D1_6F;
const WAIT: Duration = Duration::from_secs(5);

/// A scratch directory for one engine's runtime and config dirs, removed afterwards.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("glint-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(dir.join("run")).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Engine {
    x: Xvfb,
    scratch: Scratch,
    child: Child,
    socket: PathBuf,
}

impl Engine {
    fn start() -> Option<Self> {
        let x = match Xvfb::start(640, 480) {
            Ok(x) => x,
            Err(SandboxError::NotInstalled) => {
                eprintln!("skipped: Xvfb is not installed");
                return None;
            }
            Err(e) => panic!("{e}"),
        };
        let scratch = Scratch::new();
        let child = spawn(&x, &scratch);
        let socket = scratch
            .0
            .join("run/glint")
            .join(format!("glintd-{}.sock", x.display().number));
        let deadline = Instant::now() + WAIT;
        while !socket.exists() {
            assert!(Instant::now() < deadline, "glintd didn't create its socket");
            std::thread::sleep(Duration::from_millis(20));
        }
        Some(Self {
            x,
            scratch,
            child,
            socket,
        })
    }

    fn client(&self) -> Client {
        let mut c = Client::connect(&self.socket).unwrap();
        assert!(matches!(
            c.hello(ClientKind::Ctl, WAIT).unwrap(),
            Event::HelloAck { protocol: 1, .. }
        ));
        c
    }

    fn wait(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "glintd didn't exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn(x: &Xvfb, scratch: &Scratch) -> Child {
    Command::new(env!("CARGO_BIN_EXE_glintd"))
        .env("DISPLAY", x.display().to_string())
        .env("XDG_RUNTIME_DIR", scratch.0.join("run"))
        .env("XDG_CONFIG_HOME", scratch.0.join("config"))
        .env("XDG_STATE_HOME", scratch.0.join("state"))
        .stdin(Stdio::null())
        .spawn()
        .unwrap()
}

/// A full-screen click target under the pointer, and a small window at the center that shows the
/// color while mapped.
struct Screen {
    conn: RustConnection,
    clicks: u32,
    color: u32,
}

impl Screen {
    fn open(x: &Xvfb) -> Self {
        let conn = conn::open(x.display()).unwrap();
        let root = conn.setup().roots[0].root;
        let clicks = conn.generate_id().unwrap();
        // A background, so the area the color window uncovers is repainted (without one, the
        // green would stay in the screen image after the color window is unmapped).
        let aux = CreateWindowAux::new()
            .background_pixel(0x10_10_10)
            .event_mask(EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE)
            .override_redirect(1);
        conn.create_window(0, clicks, root, 0, 0, 640, 480, 0, WindowClass::INPUT_OUTPUT, 0, &aux)
            .unwrap();
        conn.map_window(clicks).unwrap();
        conn.warp_pointer(x11rb::NONE, clicks, 0, 0, 0, 0, 20, 20).unwrap();
        conn.set_input_focus(InputFocus::PARENT, clicks, x11rb::CURRENT_TIME)
            .unwrap();
        let color = conn.generate_id().unwrap();
        let aux = CreateWindowAux::new().background_pixel(GREEN).override_redirect(1);
        conn.create_window(0, color, root, 300, 220, 40, 40, 0, WindowClass::INPUT_OUTPUT, 0, &aux)
            .unwrap();
        conn.sync().unwrap();
        Self { conn, clicks, color }
    }

    fn show(&self, on: bool) {
        if on {
            self.conn.map_window(self.color).unwrap();
        } else {
            self.conn.unmap_window(self.color).unwrap();
        }
        self.conn.sync().unwrap();
    }

    fn presses(&self, ms: u64) -> (u32, u32) {
        let (mut press, mut release) = (0, 0);
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            match self.conn.poll_for_event().unwrap() {
                Some(XEvent::ButtonPress(e)) if e.event == self.clicks => press += 1,
                Some(XEvent::ButtonRelease(e)) if e.event == self.clicks => release += 1,
                Some(_) => {}
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        }
        (press, release)
    }
}

fn green_profile() -> Profile {
    let mut p = Profile::new("Green");
    p.color.include = vec![ShadePreset::Normal.entry(ColorModel::Oklch, Rgb::from_u32(GREEN))];
    p
}

#[test]
fn watching_and_clicking_through_the_socket() {
    let Some(mut engine) = Engine::start() else { return };
    let screen = Screen::open(&engine.x);
    let mut c = engine.client();

    let Event::State(s) = c.call(Request::GetState, WAIT).unwrap() else {
        panic!()
    };
    assert_eq!(
        (s.started, s.phase, s.profile.as_str()),
        (false, Phase::Stopped, "Default")
    );
    assert_eq!(s.cannot_start.as_deref(), Some("Pick a color to start"));
    match c.call(Request::Start, WAIT).unwrap() {
        Event::Error { code, message, .. } => assert_eq!(
            (code.as_str(), message.as_str()),
            ("cannot_start", "Pick a color to start")
        ),
        other => panic!("{other:?}"),
    }
    // A bad profile names the field.
    let mut bad = green_profile();
    bad.sampling.k_enter = Some(10);
    match c.call(Request::SetProfile { profile: Box::new(bad) }, WAIT).unwrap() {
        Event::Error { field, .. } => assert_eq!(field.as_deref(), Some("sampling.k_enter")),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        c.call(
            Request::SetProfile {
                profile: Box::new(green_profile())
            },
            WAIT
        )
        .unwrap(),
        Event::ProfileApplied { .. }
    ));
    let Event::Classified { verdicts } = c
        .call(
            Request::Classify {
                colors: vec![Rgb::from_u32(GREEN), Rgb::new(0, 0, 255)],
            },
            WAIT,
        )
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(verdicts, [Class::Tight, Class::Absent]);

    let Event::State(s) = c.call(Request::Start, WAIT).unwrap() else {
        panic!()
    };
    assert_eq!(
        (s.started, s.phase, s.profile.as_str()),
        (true, Phase::Watching, "Green")
    );
    assert_eq!(screen.presses(150), (0, 0));
    for _ in 0..3 {
        screen.show(true);
        assert_eq!(screen.presses(150), (1, 1));
        screen.show(false);
        assert_eq!(screen.presses(100), (0, 0));
    }

    // Telemetry and the live swatch arrive once subscribed.
    c.send(Request::Subscribe {
        telemetry_hz: 10,
        live_hz: 20,
    })
    .unwrap();
    let (mut telemetry, mut live) = (None, false);
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && !(telemetry.is_some() && live) {
        match c.next_event(WAIT).unwrap().body {
            Event::Telemetry(t) => telemetry = Some(t),
            Event::Live { .. } => live = true,
            _ => {}
        }
    }
    let t = telemetry.unwrap();
    assert!(live && t.clicks == 3 && t.checks_per_sec > 1000.0, "{t:?}");

    assert!(matches!(c.call(Request::Quit, WAIT).unwrap(), Event::Bye));
    assert!(engine.wait().success());
    assert!(!engine.socket.exists());
}

#[test]
fn starting_while_present_waits_and_sigterm_exits_cleanly() {
    let Some(mut engine) = Engine::start() else { return };
    let screen = Screen::open(&engine.x);
    let mut c = engine.client();
    assert!(matches!(
        c.call(
            Request::SetProfile {
                profile: Box::new(green_profile())
            },
            WAIT
        )
        .unwrap(),
        Event::ProfileApplied { .. }
    ));
    screen.show(true);
    assert!(matches!(c.call(Request::Start, WAIT).unwrap(), Event::State(s) if s.started));
    assert_eq!(screen.presses(250), (0, 0)); // arm-while-present
    screen.show(false);
    assert_eq!(screen.presses(100), (0, 0));
    screen.show(true);
    assert_eq!(screen.presses(100), (1, 1));

    // A second engine on the same display is refused with exit code 3.
    let mut second = spawn(&engine.x, &engine.scratch);
    let deadline = Instant::now() + WAIT;
    let status = loop {
        if let Some(s) = second.try_wait().unwrap() {
            break s;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(3));

    rustix::process::kill_process(
        rustix::process::Pid::from_child(&engine.child),
        rustix::process::Signal::TERM,
    )
    .unwrap();
    assert!(engine.wait().success());
    assert!(!engine.socket.exists());
}

fn ctl(engine_socket: &std::path::Path, display: &str, args: &[&str]) -> Result<String, glint_ctl::CtlError> {
    use clap::Parser as _;
    let mut argv = vec![
        "glint-ctl",
        "--socket",
        engine_socket.to_str().unwrap(),
        "--display",
        display,
    ];
    argv.extend_from_slice(args);
    let cli = glint_ctl::Cli::parse_from(argv);
    let mut out = Vec::new();
    glint_ctl::execute(&cli, &mut out)?;
    Ok(String::from_utf8(out).unwrap())
}

#[test]
fn glint_ctl_drives_the_engine() {
    let Some(mut engine) = Engine::start() else { return };
    let screen = Screen::open(&engine.x);
    let display = engine.x.display().to_string();
    let sock = engine.socket.clone();

    let status = ctl(&sock, &display, &["status"]).unwrap();
    assert!(status.starts_with("Engine running (pid "), "{status}");
    assert!(status.contains("can't start yet: Pick a color to start"), "{status}");

    let file = engine.scratch.0.join("green.toml");
    std::fs::write(&file, green_profile().to_toml().unwrap()).unwrap();
    let imported = ctl(&sock, &display, &["profile", "import", file.to_str().unwrap()]).unwrap();
    assert!(imported.starts_with("Imported \"Green\""), "{imported}");
    assert_eq!(
        ctl(&sock, &display, &["profile", "list"]).unwrap(),
        "  Default\n* Green\n"
    );
    let shown = ctl(&sock, &display, &["profile", "show"]).unwrap();
    assert_eq!(Profile::load(&shown).unwrap().value, green_profile());
    assert_eq!(
        ctl(&sock, &display, &["classify", "#3BD16F", "#0000FF"]).unwrap(),
        "#3BD16F tight\n#0000FF absent\n"
    );
    assert!(ctl(&sock, &display, &["profile", "use", "Nope"]).is_err());

    assert!(
        ctl(&sock, &display, &["start"])
            .unwrap()
            .starts_with("watching · profile Green")
    );
    assert_eq!(screen.presses(100), (0, 0));
    screen.show(true);
    assert_eq!(screen.presses(100), (1, 1));
    let panicked = ctl(&sock, &display, &["panic"]).unwrap();
    assert!(
        panicked.contains("stopped") && panicked.contains("last stop: Emergency stop."),
        "{panicked}"
    );
    // Start is ignored for half a second after an emergency stop.
    let early = ctl(&sock, &display, &["start"]).unwrap_err().to_string();
    assert!(early.contains("Emergency stop"), "{early}");
    assert_eq!(
        ctl(&sock, &display, &["release-all"]).unwrap(),
        "Released every button and key.\n"
    );
    assert_eq!(ctl(&sock, &display, &["quit"]).unwrap(), "The engine stopped.\n");
    assert!(engine.wait().success());
}

#[test]
fn release_all_works_without_an_engine() {
    use x11rb::protocol::xtest::ConnectionExt as _;
    let x = match Xvfb::start(640, 480) {
        Ok(x) => x,
        Err(SandboxError::NotInstalled) => return eprintln!("skipped: Xvfb is not installed"),
        Err(e) => panic!("{e}"),
    };
    let screen = Screen::open(&x);
    // Something pressed button 1 and died without releasing it.
    let stuck = conn::open(x.display()).unwrap();
    let root = stuck.setup().roots[0].root;
    stuck.xtest_fake_input(4, 1, 0, root, 0, 0, 0).unwrap();
    stuck.sync().unwrap();
    assert_eq!(screen.presses(50), (1, 0));
    let nowhere = std::env::temp_dir().join("glint-no-such-engine.sock");
    let out = ctl(&nowhere, &x.display().to_string(), &["release-all"]).unwrap();
    assert!(out.starts_with("No engine is running; released"), "{out}");
    assert_eq!(screen.presses(50), (0, 1));
    let none = ctl(&nowhere, &x.display().to_string(), &["status"]).unwrap_err();
    assert!(matches!(none, glint_ctl::CtlError::NoEngine(_)), "{none}");
}

#[test]
fn profiles_and_settings_survive_a_restart() {
    let Some(mut engine) = Engine::start() else { return };
    let mut c = engine.client();
    let mut p = green_profile();
    p.name = "Sober — green flash".into();
    p.action.hold_ms = 25;
    assert!(matches!(
        c.call(
            Request::SetProfile {
                profile: Box::new(p.clone())
            },
            WAIT
        )
        .unwrap(),
        Event::ProfileApplied { .. }
    ));
    let settings = glint_core::Settings {
        close_policy: glint_core::settings::ClosePolicy::KeepRunning,
        ..glint_core::Settings::default()
    };
    assert!(matches!(
        c.call(
            Request::SetSettings {
                settings: Box::new(settings)
            },
            WAIT
        )
        .unwrap(),
        Event::SettingsApplied
    ));
    assert!(matches!(c.call(Request::Quit, WAIT).unwrap(), Event::Bye));
    assert!(engine.wait().success());

    let config = engine.scratch.0.join("config/glint");
    assert!(config.join("profiles/sober-green-flash.toml").exists());
    let saved = std::fs::read_to_string(config.join("settings.toml")).unwrap();
    assert!(
        saved.contains("active_profile = \"Sober — green flash\"") && saved.contains("keep-running"),
        "{saved}"
    );

    engine.child = spawn(&engine.x, &engine.scratch);
    let deadline = Instant::now() + WAIT;
    while !engine.socket.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut c = engine.client();
    let Event::State(s) = c.call(Request::GetState, WAIT).unwrap() else {
        panic!()
    };
    assert_eq!(s.profile, "Sober — green flash");
    let Event::Profile { profile } = c.call(Request::GetProfile { name: None }, WAIT).unwrap() else {
        panic!()
    };
    assert_eq!(*profile, p);
}
