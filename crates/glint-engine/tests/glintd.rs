// SPDX-License-Identifier: MIT
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

    /// Waits until at least `want` presses and releases have arrived, for up to 2 s, then 30 ms
    /// more to catch any extra. A fixed window flakes on a busy machine.
    fn wait_for(&self, want: (u32, u32)) -> (u32, u32) {
        let (mut press, mut release) = (0, 0);
        let deadline = Instant::now() + Duration::from_secs(2);
        while (press < want.0 || release < want.1) && Instant::now() < deadline {
            let (p, r) = self.presses(5);
            (press, release) = (press + p, release + r);
        }
        let (p, r) = self.presses(30);
        (press + p, release + r)
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
    assert_eq!(screen.presses(300), (0, 0));
    for _ in 0..3 {
        screen.show(true);
        assert_eq!(screen.wait_for((1, 1)), (1, 1));
        screen.show(false);
        assert_eq!(screen.presses(300), (0, 0));
    }

    // Telemetry and the live swatch arrive once subscribed.
    c.send(Request::Subscribe {
        telemetry_hz: 10,
        live_hz: 20,
        trace: false,
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
    assert_eq!(screen.presses(300), (0, 0));
    screen.show(true);
    assert_eq!(screen.wait_for((1, 1)), (1, 1));

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
    assert_eq!(screen.presses(300), (0, 0));
    screen.show(true);
    assert_eq!(screen.wait_for((1, 1)), (1, 1));
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
    assert_eq!(screen.wait_for((1, 0)), (1, 0));
    let nowhere = std::env::temp_dir().join("glint-no-such-engine.sock");
    let out = ctl(&nowhere, &x.display().to_string(), &["release-all"]).unwrap();
    assert!(out.starts_with("No engine is running; released"), "{out}");
    assert_eq!(screen.wait_for((0, 1)), (0, 1));
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

/// Xvfb's own keyboard, which isn't an XTEST device.
fn physical_keyboard(conn: &RustConnection) -> u8 {
    use x11rb::protocol::xinput::{ConnectionExt as _, DeviceType};
    let devices = conn.xinput_xi_query_device(0u16).unwrap().reply().unwrap();
    let d = devices
        .infos
        .iter()
        .find(|d| d.type_ == DeviceType::SLAVE_KEYBOARD && !String::from_utf8_lossy(&d.name).contains("XTEST"))
        .unwrap();
    u8::try_from(d.deviceid).unwrap()
}

/// A key typed "on" `device`: XTest with an XInput device-key event type, so the hotkey filter
/// sees a real keyboard as the source.
fn physical_key(conn: &RustConnection, device: u8, keycode: u8, press: bool) {
    use x11rb::connection::RequestConnection as _;
    use x11rb::protocol::xinput;
    use x11rb::protocol::xtest::ConnectionExt as _;
    let root = conn.setup().roots[0].root;
    let base = conn
        .extension_information(xinput::X11_EXTENSION_NAME)
        .unwrap()
        .unwrap()
        .first_event;
    conn.xtest_fake_input(base + if press { 1 } else { 2 }, keycode, 0, root, 0, 0, device)
        .unwrap();
    conn.sync().unwrap();
}

fn tap(conn: &RustConnection, device: u8, keycode: u8) {
    physical_key(conn, device, keycode, true);
    physical_key(conn, device, keycode, false);
}

fn started(c: &mut Client) -> bool {
    std::thread::sleep(Duration::from_millis(50));
    match c.call(Request::GetState, WAIT).unwrap() {
        Event::State(s) => s.started,
        other => panic!("{other:?}"),
    }
}

#[test]
fn hotkeys_start_stop_and_sample() {
    use glint_x11::keymap::{Keymap, keysym_from_name};
    use x11rb::protocol::xtest::ConnectionExt as _;

    let Some(engine) = Engine::start() else { return };
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
    let keys = conn::open(engine.x.display()).unwrap();
    let keyboard = physical_keyboard(&keys);
    let map = Keymap::fetch(&keys).unwrap();
    let code = |name: &str| map.keycode(keysym_from_name(name).unwrap()).unwrap();
    let (f6, f7, f8) = (code("F6"), code("F7"), code("F8"));

    // F7 made through the XTEST device (the way Glint's own keys are made) is ignored.
    let root = keys.setup().roots[0].root;
    keys.xtest_fake_input(2, f7, 0, root, 0, 0, 0).unwrap();
    keys.xtest_fake_input(3, f7, 0, root, 0, 0, 0).unwrap();
    keys.sync().unwrap();
    assert!(!started(&mut c));

    // F7 on a real keyboard starts watching; the engine clicks; F7 again stops.
    tap(&keys, keyboard, f7);
    assert!(started(&mut c));
    assert_eq!(screen.presses(300), (0, 0));
    screen.show(true);
    assert_eq!(screen.wait_for((1, 1)), (1, 1));
    tap(&keys, keyboard, f7);
    assert!(!started(&mut c));

    // F8 stops at once and Start is ignored for half a second.
    tap(&keys, keyboard, f7);
    assert!(started(&mut c));
    tap(&keys, keyboard, f8);
    std::thread::sleep(Duration::from_millis(50)); // the key travels through the X server first
    let Event::State(s) = c.call(Request::GetState, WAIT).unwrap() else {
        panic!()
    };
    assert!(!s.started && s.last_stop.as_deref() == Some("Emergency stop."), "{s:?}");
    tap(&keys, keyboard, f7);
    assert!(!started(&mut c));
    std::thread::sleep(Duration::from_millis(500));
    tap(&keys, keyboard, f7);
    assert!(started(&mut c));

    // F6 samples the color at the watched spot, even while stopped.
    tap(&keys, keyboard, f7);
    assert!(!started(&mut c));
    tap(&keys, keyboard, f6);
    let deadline = Instant::now() + WAIT;
    let sampled = loop {
        assert!(Instant::now() < deadline, "no color_sampled");
        if let Event::ColorSampled { rgb, .. } = c.next_event(WAIT).unwrap().body {
            break rgb;
        }
    };
    assert_eq!(sampled, Rgb::from_u32(GREEN));

    // Hold to start: watching only while F7 is held.
    let settings = glint_core::Settings {
        active_profile: "Green".into(),
        hotkeys: glint_core::settings::Hotkeys {
            mode: glint_core::settings::HotkeyMode::Hold,
            ..Default::default()
        },
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
    physical_key(&keys, keyboard, f7, true);
    assert!(started(&mut c));
    physical_key(&keys, keyboard, f7, false);
    assert!(!started(&mut c));
}

/// The next event `want` picks out, skipping others (at most 5 s).
fn wait_event<T>(c: &mut Client, mut want: impl FnMut(Event) -> Option<T>) -> T {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if let Some(t) = want(c.next_event(WAIT).unwrap().body) {
            return t;
        }
    }
    panic!("the event never came");
}

#[test]
fn picking_a_color_with_the_mouse() {
    use x11rb::protocol::xtest::ConnectionExt as _;
    let Some(engine) = Engine::start() else { return };
    let screen = Screen::open(&engine.x);
    screen.show(true); // green at (300, 220)–(340, 260)
    let mut c = engine.client();
    assert!(matches!(c.call(Request::PickColor, WAIT).unwrap(), Event::PickStarted));

    // The pointer moves over the green: a magnifier frame with the green at its center.
    let x = conn::open(engine.x.display()).unwrap();
    let root = x.setup().roots[0].root;
    x.xtest_fake_input(6, 0, 0, root, 320, 240, 0).unwrap(); // MotionNotify, absolute
    x.sync().unwrap();
    // The first frame may still show where the pointer was when the pick started.
    let (size, pixels) = wait_event(&mut c, |e| match e {
        Event::Magnifier { size, pixels, center } if center == Rgb::from_u32(GREEN) => Some((size, pixels)),
        _ => None,
    });
    assert_eq!((size, pixels.len()), (11, 121));
    // Near the square's edge, the frame shows both sides of it.
    x.xtest_fake_input(6, 0, 0, root, 300, 240, 0).unwrap();
    x.sync().unwrap();
    wait_event(&mut c, |e| match e {
        Event::Magnifier { pixels, .. }
            if pixels.contains(&Rgb::from_u32(GREEN)) && pixels.contains(&Rgb::from_u32(0x10_10_10)) =>
        {
            Some(())
        }
        _ => None,
    });

    // A left click picks the color under the pointer, and lets the pointer go.
    x.xtest_fake_input(6, 0, 0, root, 330, 250, 0).unwrap();
    x.xtest_fake_input(4, 1, 0, root, 0, 0, 0).unwrap();
    x.xtest_fake_input(5, 1, 0, root, 0, 0, 0).unwrap();
    x.sync().unwrap();
    let rgb = wait_event(&mut c, |e| match e {
        Event::ColorPicked { rgb, .. } => Some(rgb),
        _ => None,
    });
    assert_eq!(rgb, Rgb::from_u32(GREEN));

    // A pick over the click-counting window: the click goes to the engine's grab, not to it.
    assert!(matches!(c.call(Request::PickColor, WAIT).unwrap(), Event::PickStarted));
    x.xtest_fake_input(6, 0, 0, root, 100, 100, 0).unwrap();
    x.xtest_fake_input(4, 1, 0, root, 0, 0, 0).unwrap();
    x.xtest_fake_input(5, 1, 0, root, 0, 0, 0).unwrap();
    x.sync().unwrap();
    let rgb = wait_event(&mut c, |e| match e {
        Event::ColorPicked { rgb, .. } => Some(rgb),
        _ => None,
    });
    assert_eq!(rgb, Rgb::from_u32(0x10_10_10));
    assert_eq!(screen.presses(100), (0, 0));

    // Picking again works; a right click cancels, and so does a request.
    assert!(matches!(c.call(Request::PickColor, WAIT).unwrap(), Event::PickStarted));
    x.xtest_fake_input(4, 3, 0, root, 0, 0, 0).unwrap();
    x.xtest_fake_input(5, 3, 0, root, 0, 0, 0).unwrap();
    x.sync().unwrap();
    wait_event(&mut c, |e| matches!(e, Event::PickCancelled).then_some(()));
    assert!(matches!(c.call(Request::PickColor, WAIT).unwrap(), Event::PickStarted));
    assert!(matches!(
        c.call(Request::CancelPick, WAIT).unwrap(),
        Event::PickCancelled
    ));
    // The pointer is free: clicks reach the window again.
    x.xtest_fake_input(4, 1, 0, root, 0, 0, 0).unwrap();
    x.xtest_fake_input(5, 1, 0, root, 0, 0, 0).unwrap();
    x.sync().unwrap();
    assert_eq!(screen.wait_for((1, 1)), (1, 1));
    // No picking while watching.
    let mut p = green_profile();
    p.name = "Green".into();
    c.call(Request::SetProfile { profile: Box::new(p) }, WAIT).unwrap();
    screen.show(false);
    c.call(Request::Start, WAIT).unwrap();
    assert!(matches!(c.call(Request::PickColor, WAIT).unwrap(), Event::Error { .. }));
}

#[test]
fn a_mouse_side_button_as_a_hotkey() {
    use x11rb::protocol::xtest::ConnectionExt as _;
    let Some(engine) = Engine::start() else { return };
    let _screen = Screen::open(&engine.x);
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
    let mut settings = glint_core::Settings::default();
    settings.hotkeys.start_stop = "Mouse4".into();
    settings.hotkeys.mode = glint_core::settings::HotkeyMode::Hold;
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
    let x = conn::open(engine.x.display()).unwrap();
    // Xvfb's mouse has three buttons, so a "physical" button 8 can't be faked here: glint-x11's
    // Xvfb tests check raw buttons from a real device, and the matcher's tests check the binding.
    // What this checks end to end: the engine takes Mouse4 as a hotkey (no warning) and binds it.
    // Button 8 from XTest itself, the way Glint's own Back clicks arrive, never counts.
    let root = x.setup().roots[0].root;
    x.xtest_fake_input(4, 8, 0, root, 0, 0, 0).unwrap();
    x.sync().unwrap();
    assert!(!started(&mut c));
    x.xtest_fake_input(5, 8, 0, root, 0, 0, 0).unwrap();
    x.sync().unwrap();
}

/// The window-buffer self-check (§5.2, ADR 0014): behind another window it can only test for a
/// blank image; once the window is in front it compares with the screen.
#[test]
fn the_window_buffer_self_check_compares_once_the_window_is_in_front() {
    use glint_core::pattern::Rect;
    use glint_core::profile::{Backend, TargetMode};

    let Some(engine) = Engine::start() else { return };
    let display = engine.x.display();
    let game = glint_lab::Canvas::game(display, Rect::new(40, 40, 300, 200), "glint-test-game").unwrap();
    let other = glint_lab::Canvas::game(display, Rect::new(360, 40, 200, 200), "glint-test-other").unwrap();
    other.focus().unwrap();
    let mut c = engine.client();
    let mut p = green_profile();
    p.target.mode = TargetMode::Window;
    p.target.window.class = "glint-test-game".into();
    p.sampling.backend = Backend::WindowBuffer;
    assert!(matches!(
        c.call(Request::SetProfile { profile: Box::new(p) }, WAIT).unwrap(),
        Event::ProfileApplied { .. }
    ));
    let state = |c: &mut Client| match c.call(Request::GetState, WAIT).unwrap() {
        Event::State(s) => (s.backend, s.backend_reason.unwrap_or_default()),
        other => panic!("{other:?}"),
    };
    let (backend, why) = state(&mut c);
    assert_eq!(backend, "window-buffer");
    assert!(why.contains("next in front"), "{why}");
    game.focus().unwrap();
    let deadline = Instant::now() + WAIT;
    loop {
        let (backend, why) = state(&mut c);
        if why.contains("matched the screen in 20 of 20 reads") {
            assert_eq!(backend, "window-buffer");
            break;
        }
        assert!(Instant::now() < deadline, "never compared: {why}");
        std::thread::sleep(Duration::from_millis(50));
    }
}
