// SPDX-License-Identifier: GPL-3.0-or-later
//! Phase 0 spike (PLAN §19): an XTest click into a test window, timed from
//! sending to arrival, plus the release semantics the engine relies on:
//!
//! 1. press/release latency for a mouse button and a key (send → event arrives);
//! 2. a release for something that isn't held produces no event (defensive release, §4.3);
//! 3. a press sent on one connection can be released from another (connection D, §4.1);
//! 4. `XTestGrabControl(impervious)` keeps injecting while another client grabs the server (§6);
//! 5. a 0 ms hold (press and release in one flush) arrives as press then release.
//!
//! **Sandbox only.** It refuses to run on display :0; use Xvfb or Xephyr:
//! `xvfb-run -a -s "-screen 0 1280x720x24" spike-inject` or `DISPLAY=:2 spike-inject`.
#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use x11rb::CURRENT_TIME;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ConnectionExt as _, CreateWindowAux, EventMask, InputFocus,
    KEY_PRESS_EVENT, KEY_RELEASE_EVENT, Window, WindowClass,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Keycode 26 is "e" with the evdev keymap; the keysym doesn't matter for timing.
const KEY: u8 = 26;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    ButtonPress,
    ButtonRelease,
    KeyPress,
    KeyRelease,
}

/// The receiving side: a mapped window under the pointer with input focus.
struct Receiver {
    conn: RustConnection,
    win: Window,
}

impl Receiver {
    fn open() -> Res<(Self, Window)> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen_num].root;
        let win = conn.generate_id()?;
        let mask = EventMask::BUTTON_PRESS
            | EventMask::BUTTON_RELEASE
            | EventMask::KEY_PRESS
            | EventMask::KEY_RELEASE
            | EventMask::STRUCTURE_NOTIFY;
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            win,
            root,
            0,
            0,
            400,
            300,
            0,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new().background_pixel(0x0020_2020).event_mask(mask),
        )?;
        conn.map_window(win)?;
        conn.flush()?;
        loop {
            if let Event::MapNotify(e) = conn.wait_for_event()?
                && e.window == win
            {
                break;
            }
        }
        conn.warp_pointer(x11rb::NONE, win, 0, 0, 0, 0, 200, 150)?;
        conn.set_input_focus(InputFocus::PARENT, win, CURRENT_TIME)?;
        conn.sync()?;
        Ok((Self { conn, win }, root))
    }

    fn classify(&self, ev: &Event) -> Option<(Kind, u8)> {
        match ev {
            Event::ButtonPress(e) if e.event == self.win => Some((Kind::ButtonPress, e.detail)),
            Event::ButtonRelease(e) if e.event == self.win => Some((Kind::ButtonRelease, e.detail)),
            Event::KeyPress(e) if e.event == self.win => Some((Kind::KeyPress, e.detail)),
            Event::KeyRelease(e) if e.event == self.win => Some((Kind::KeyRelease, e.detail)),
            _ => None,
        }
    }

    /// Block until the next input event on the window.
    fn next(&self) -> Res<(Kind, u8)> {
        loop {
            let ev = self.conn.wait_for_event()?;
            if let Some(k) = self.classify(&ev) {
                return Ok(k);
            }
        }
    }

    /// Collect every input event that arrives within `window`.
    fn drain_for(&self, window: Duration) -> Res<Vec<(Kind, u8)>> {
        let deadline = Instant::now() + window;
        let mut out = Vec::new();
        while Instant::now() < deadline {
            match self.conn.poll_for_event()? {
                Some(ev) => out.extend(self.classify(&ev)),
                None => std::thread::sleep(Duration::from_micros(200)),
            }
        }
        Ok(out)
    }
}

struct Injector {
    conn: RustConnection,
    root: Window,
}

impl Injector {
    fn open(root: Window, impervious: bool) -> Res<Self> {
        let (conn, _) = x11rb::connect(None)?;
        let v = conn.xtest_get_version(2, 2)?.reply()?;
        if (v.major_version, v.minor_version) < (2, 2) {
            return Err(format!("XTEST {}.{} is too old", v.major_version, v.minor_version).into());
        }
        if impervious {
            conn.xtest_grab_control(true)?;
            conn.sync()?;
        }
        Ok(Self { conn, root })
    }

    fn fake(&self, ty: u8, detail: u8) -> Res<()> {
        self.conn
            .xtest_fake_input(ty, detail, CURRENT_TIME, self.root, 0, 0, 0)?;
        Ok(())
    }

    fn send(&self, ty: u8, detail: u8) -> Res<()> {
        self.fake(ty, detail)?;
        self.conn.flush()?;
        Ok(())
    }
}

fn percentiles(mut v: Vec<u32>) -> String {
    v.sort_unstable();
    let n = v.len();
    let at = |q: f64| f64::from(v[((n as f64 * q) as usize).min(n - 1)]) / 1000.0;
    format!(
        "n {n} · p50 {:.1} · p90 {:.1} · p99 {:.1} · p99.9 {:.1} · max {:.1} µs",
        at(0.5),
        at(0.9),
        at(0.99),
        at(0.999),
        at(1.0)
    )
}

fn timed(rx: &Receiver, tx: &Injector, ty: u8, detail: u8, want: Kind) -> Res<u32> {
    let t0 = Instant::now();
    tx.send(ty, detail)?;
    let got = rx.next()?;
    let ns = u32::try_from(t0.elapsed().as_nanos()).unwrap_or(u32::MAX);
    if got != (want, detail) {
        return Err(format!("expected {want:?} {detail}, got {got:?}").into());
    }
    Ok(ns)
}

fn run(count: usize) -> Res<()> {
    let (rx, root) = Receiver::open()?;
    let tx = Injector::open(root, true)?;

    // 1. Latency: send → arrival on the receiver's connection.
    let (mut bp, mut br, mut kp, mut kr) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for _ in 0..count {
        bp.push(timed(&rx, &tx, BUTTON_PRESS_EVENT, 1, Kind::ButtonPress)?);
        br.push(timed(&rx, &tx, BUTTON_RELEASE_EVENT, 1, Kind::ButtonRelease)?);
        kp.push(timed(&rx, &tx, KEY_PRESS_EVENT, KEY, Kind::KeyPress)?);
        kr.push(timed(&rx, &tx, KEY_RELEASE_EVENT, KEY, Kind::KeyRelease)?);
    }
    println!("button press   {}", percentiles(bp));
    println!("button release {}", percentiles(br));
    println!("key press      {}", percentiles(kp));
    println!("key release    {}", percentiles(kr));
    let stray = rx.drain_for(Duration::from_millis(100))?;
    println!("stray events after the latency runs: {}", stray.len());

    // 2. Releases for things that aren't held must produce nothing.
    for b in [1u8, 2, 3, 8, 9] {
        tx.fake(BUTTON_RELEASE_EVENT, b)?;
    }
    for k in 8u8..=255 {
        tx.fake(KEY_RELEASE_EVENT, k)?;
    }
    tx.conn.flush()?;
    let got = rx.drain_for(Duration::from_millis(200))?;
    println!(
        "releases of 5 buttons and keycodes 8–255 that were not held → {} event(s) {}",
        got.len(),
        if got.is_empty() { "OK (no-op)" } else { "UNEXPECTED" }
    );

    // 3. Press on connection B, release from connection D.
    let d = Injector::open(root, false)?;
    tx.send(BUTTON_PRESS_EVENT, 1)?;
    let pressed = rx.next()?;
    d.send(BUTTON_RELEASE_EVENT, 1)?;
    let released = rx.drain_for(Duration::from_millis(200))?;
    println!(
        "press on B {pressed:?}, release from D → {released:?} {}",
        if released == [(Kind::ButtonRelease, 1)] {
            "OK"
        } else {
            "UNEXPECTED"
        }
    );
    tx.send(KEY_PRESS_EVENT, KEY)?;
    let pressed = rx.next()?;
    d.send(KEY_RELEASE_EVENT, KEY)?;
    let released = rx.drain_for(Duration::from_millis(200))?;
    println!(
        "key press on B {pressed:?}, release from D → {released:?} {}",
        if released == [(Kind::KeyRelease, KEY)] {
            "OK"
        } else {
            "UNEXPECTED"
        }
    );

    // 4. Another client grabs the server; the impervious injector must still get through.
    for (label, injector) in [("impervious", &tx), ("not impervious", &d)] {
        let (grabber, _) = x11rb::connect(None)?;
        grabber.grab_server()?;
        grabber.sync()?;
        let t0 = Instant::now();
        injector.send(BUTTON_PRESS_EVENT, 1)?;
        let during = rx.drain_for(Duration::from_millis(300))?;
        let during_ms = t0.elapsed().as_secs_f64() * 1000.0;
        grabber.ungrab_server()?;
        grabber.sync()?;
        let after = rx.drain_for(Duration::from_millis(200))?;
        println!(
            "server grabbed by another client, {label} injector: during grab ({during_ms:.0} ms) {during:?} · after ungrab {after:?}"
        );
        injector.send(BUTTON_RELEASE_EVENT, 1)?;
        let _ = rx.drain_for(Duration::from_millis(100))?;
    }

    // 5. A 0 ms hold: press and release in one flush.
    tx.fake(BUTTON_PRESS_EVENT, 1)?;
    tx.fake(BUTTON_RELEASE_EVENT, 1)?;
    tx.conn.flush()?;
    let got = rx.drain_for(Duration::from_millis(200))?;
    println!(
        "0 ms hold → {got:?} {}",
        if got == [(Kind::ButtonPress, 1), (Kind::ButtonRelease, 1)] {
            "OK"
        } else {
            "UNEXPECTED"
        }
    );
    Ok(())
}

fn main() -> std::process::ExitCode {
    let display = std::env::var("DISPLAY").unwrap_or_default();
    let parsed = match x11rb_protocol::parse_display::parse_display(Some(&display)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("spike-inject: cannot parse DISPLAY={display:?}: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    if parsed.display == 0 {
        eprintln!("spike-inject: refusing to inject into display :0 (the real desktop); use Xvfb or Xephyr");
        return std::process::ExitCode::FAILURE;
    }
    let count = std::env::args().nth(1).and_then(|v| v.parse().ok()).unwrap_or(2000);
    println!("spike-inject on {display} · {count} trials per event type");
    match run(count) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("spike-inject: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
