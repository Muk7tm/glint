// SPDX-License-Identifier: GPL-3.0-or-later
//! The hot loop end to end on Xvfb: a real capture, the real trigger and a real injector. A small
//! window at the screen's center shows and hides the color; a full-screen window under the
//! pointer counts the clicks.
#![allow(clippy::unwrap_used)] // test helpers: any failure should panic

use std::sync::Arc;
use std::time::{Duration, Instant};

use glint_core::color::{ColorModel, ShadePreset};
use glint_core::profile::Profile;
use glint_core::{Lut, Rgb};
use glint_engine::controller::compile;
use glint_engine::hot::{HotEvent, HotHandle, StopReason};
use glint_x11::capture::getimage::GetImageCapture;
use glint_x11::format::PixelFormat;
use glint_x11::inject::Injector;
use glint_x11::keymap::Keymap;
use glint_x11::sandbox::{SandboxError, Xvfb};
use glint_x11::wire::RawConn;
use glint_x11::{caps, conn};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ConnectionExt as _, CreateWindowAux, EventMask, InputFocus, WindowClass};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const GREEN: u32 = 0x3B_D1_6F;

struct Stage {
    _x: Xvfb,
    conn: RustConnection,
    clicks: u32,
    color: u32,
    hot: HotHandle,
    events: rtrb::Consumer<HotEvent>,
    origin: (i16, i16),
}

impl Stage {
    fn new(mut profile: Profile) -> Option<Self> {
        let x = match Xvfb::start(640, 480) {
            Ok(x) => x,
            Err(SandboxError::NotInstalled) => {
                eprintln!("skipped: Xvfb is not installed");
                return None;
            }
            Err(e) => panic!("{e}"),
        };
        let conn = conn::open(x.display()).unwrap();
        let root = conn.setup().roots[0].root;
        // A full-screen window under the pointer receives the clicks.
        let clicks = conn.generate_id().unwrap();
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
        // A small window over the center shows the color while mapped.
        let color = conn.generate_id().unwrap();
        let aux = CreateWindowAux::new().background_pixel(GREEN).override_redirect(1);
        conn.create_window(0, color, root, 300, 220, 40, 40, 0, WindowClass::INPUT_OUTPUT, 0, &aux)
            .unwrap();
        conn.sync().unwrap();

        profile.color.include = vec![ShadePreset::Normal.entry(ColorModel::Oklch, Rgb::from_u32(GREEN))];
        let caps = caps::probe(&conn).unwrap();
        let keymap = Keymap::fetch(&conn).unwrap();
        let lut = Arc::new(Lut::build(&profile.rules()));
        let compiled = compile(&profile, &keymap, lut, (320, 240)).unwrap();
        let placement = compiled
            .pattern
            .place(glint_core::pattern::Rect::new(0, 0, 640, 480), (0, 0))
            .unwrap();
        let origin = (placement.rect.x as i16, placement.rect.y as i16);
        let (a, setup) = RawConn::connect(x.display()).unwrap();
        let capture = GetImageCapture::new(a, root, PixelFormat::root(&setup, 0).unwrap());
        let (b, _) = RawConn::connect(x.display()).unwrap();
        let injector = Injector::new(b, root, caps.xtest_opcode, compiled.action).unwrap();
        let (hot, events, _preview) = HotHandle::spawn(Box::new(capture), injector, compiled).unwrap();
        Some(Self {
            _x: x,
            conn,
            clicks,
            color,
            hot,
            events,
            origin,
        })
    }

    fn show(&self, on: bool) {
        if on {
            self.conn.map_window(self.color).unwrap();
        } else {
            self.conn.unmap_window(self.color).unwrap();
        }
        self.conn.sync().unwrap();
    }

    /// Button presses and releases the click window received within `ms`.
    fn presses(&self, ms: u64) -> (u32, u32) {
        let (mut press, mut release) = (0, 0);
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            match self.conn.poll_for_event().unwrap() {
                Some(Event::ButtonPress(e)) if e.event == self.clicks => press += 1,
                Some(Event::ButtonRelease(e)) if e.event == self.clicks => release += 1,
                Some(_) => {}
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        }
        (press, release)
    }

    fn stops(&mut self) -> Vec<StopReason> {
        let mut out = Vec::new();
        while let Ok(e) = self.events.pop() {
            if let HotEvent::Stopped { reason, .. } = e {
                out.push(reason);
            }
        }
        out
    }
}

#[test]
fn one_click_per_appearance_and_none_for_short_dropouts() {
    let Some(s) = Stage::new(Profile::new("test")) else {
        return;
    };
    s.hot.start(s.origin, true);
    assert_eq!(s.presses(100), (0, 0)); // watched absence: re-armed
    for _ in 0..3 {
        s.show(true);
        assert_eq!(s.presses(150), (1, 1), "one click, released after the 10 ms hold");
        // A 15 ms dropout is shorter than "Gone after" (40 ms): the same appearance.
        s.show(false);
        std::thread::sleep(Duration::from_millis(15));
        s.show(true);
        assert_eq!(s.presses(100), (0, 0));
        s.show(false);
        assert_eq!(s.presses(100), (0, 0));
    }
}

#[test]
fn starting_while_the_color_shows_waits_for_it_to_leave() {
    let Some(mut s) = Stage::new(Profile::new("test")) else {
        return;
    };
    s.show(true);
    s.hot.start(s.origin, true);
    assert_eq!(s.presses(200), (0, 0)); // arm-while-present: no click
    s.show(false);
    assert_eq!(s.presses(100), (0, 0));
    s.show(true);
    assert_eq!(s.presses(100), (1, 1));
    // Stopping and starting again with the color still there: no click either.
    s.hot.stop();
    s.hot.start(s.origin, true);
    assert_eq!(s.presses(200), (0, 0));
    // A closed gate (target window not in front) blocks clicks; opening it restarts latched.
    s.show(false);
    s.hot.set_gate(false);
    std::thread::sleep(Duration::from_millis(100));
    s.show(true);
    assert_eq!(s.presses(100), (0, 0));
    s.hot.set_gate(true);
    assert_eq!(s.presses(100), (0, 0));
    assert!(s.stops().is_empty());
}

#[test]
fn a_feedback_loop_trips_the_safety_stop() {
    let mut profile = Profile::new("test");
    profile.trigger.rearm_absent_ms = 0;
    profile.trigger.rearm_absent_samples = 1;
    profile.trigger.max_clicks_per_sec = 5;
    let Some(mut s) = Stage::new(profile) else { return };
    s.hot.start(s.origin, true);
    std::thread::sleep(Duration::from_millis(20));
    // Blink every 30 ms, faster than the safety stop allows.
    let mut presses = 0;
    for _ in 0..12 {
        s.show(true);
        presses += s.presses(15).0;
        s.show(false);
        presses += s.presses(15).0;
    }
    assert_eq!(presses, 5, "five clicks, then the safety stop");
    assert_eq!(s.stops(), [StopReason::SafetyStop]);
    assert!(!s.hot.control().armed);
}

#[test]
fn telemetry_counts_checks_clicks_and_the_check_rate() {
    use std::sync::Mutex;

    use glint_engine::telemetry::{self, Notice, Stats};

    let Some(mut s) = Stage::new(Profile::new("test")) else {
        return;
    };
    let events = std::mem::replace(&mut s.events, rtrb::RingBuffer::new(1).1);
    let stats = Arc::new(Mutex::new(Stats::default()));
    let (tx, rx) = std::sync::mpsc::channel();
    let shared = Arc::clone(s.hot.shared());
    let stats_for_thread = Arc::clone(&stats);
    let telemetry =
        std::thread::spawn(move || telemetry::run(events, shared, stats_for_thread, move |n| tx.send(n).unwrap()));

    s.hot.start(s.origin, true);
    assert_eq!(s.presses(100), (0, 0));
    s.show(true);
    assert_eq!(s.presses(1100), (1, 1));
    match rx.recv_timeout(Duration::from_secs(1)).unwrap() {
        Notice::Fired { count, latency } => assert!(count == 1 && latency < Duration::from_millis(5), "{latency:?}"),
        other => panic!("{other:?}"),
    }
    let snap = stats.lock().unwrap().clone();
    assert_eq!((snap.clicks, snap.dropped), (1, 0), "{snap:?}");
    // Balanced is 2,000 checks per second; leave room for a busy test machine.
    assert!(
        snap.checks_per_sec > 1500.0 && snap.checks_per_sec <= 2100.0,
        "{snap:?}"
    );
    assert!(snap.capture_p50_us > 0.0, "{snap:?}");

    s.hot.shutdown();
    telemetry.join().unwrap();
    assert!(rx.try_iter().any(|n| n == Notice::Exited));
}
