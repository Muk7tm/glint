// SPDX-License-Identifier: GPL-3.0-or-later
//! A panic anywhere in the engine releases held input on connection D first (§4.3). The panic hook
//! is process-wide, so this test has a process of its own.

use std::sync::Arc;
use std::time::{Duration, Instant};

use glint_engine::lifecycle::{Emergency, install_panic_hook};
use glint_x11::caps;
use glint_x11::conn;
use glint_x11::inject::{Action, BUTTON_PRESS, BUTTON_RELEASE, Injector, Input};
use glint_x11::sandbox::{SandboxError, Xvfb};
use glint_x11::wire::RawConn;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ConnectionExt as _, CreateWindowAux, EventMask, InputFocus, WindowClass};
use x11rb::wrapper::ConnectionExt as _;

#[test]
fn a_panic_releases_the_held_button() {
    let x = match Xvfb::start(640, 480) {
        Ok(x) => x,
        Err(SandboxError::NotInstalled) => return eprintln!("skipped: Xvfb is not installed"),
        Err(e) => panic!("{e}"),
    };
    // A window under the pointer that records button events.
    let rx = conn::open(x.display()).unwrap();
    let root = rx.setup().roots[0].root;
    let win = rx.generate_id().unwrap();
    let aux = CreateWindowAux::new()
        .event_mask(EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE)
        .override_redirect(1);
    rx.create_window(0, win, root, 0, 0, 200, 200, 0, WindowClass::INPUT_OUTPUT, 0, &aux)
        .unwrap();
    rx.map_window(win).unwrap();
    rx.warp_pointer(x11rb::NONE, win, 0, 0, 0, 0, 100, 100).unwrap();
    rx.set_input_focus(InputFocus::PARENT, win, x11rb::CURRENT_TIME)
        .unwrap();
    rx.sync().unwrap();

    let c = conn::open(x.display()).unwrap();
    let caps = caps::probe(&c).unwrap();
    let (b, _) = RawConn::connect(x.display()).unwrap();
    let mut injector = Injector::new(
        b,
        root,
        caps.xtest_opcode,
        Action::new(Input::Button(1), Duration::from_secs(60)),
    )
    .unwrap();
    install_panic_hook(Arc::new(Emergency::new(conn::open(x.display()).unwrap(), root, vec![])));

    injector.press(Instant::now()).unwrap();
    let result = std::thread::spawn(|| panic!("a bug in the engine")).join();
    assert!(result.is_err());

    let mut got = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline && got.len() < 2 {
        match rx.poll_for_event().unwrap() {
            Some(Event::ButtonPress(e)) => got.push((BUTTON_PRESS, e.detail)),
            Some(Event::ButtonRelease(e)) => got.push((BUTTON_RELEASE, e.detail)),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    assert_eq!(got, [(BUTTON_PRESS, 1), (BUTTON_RELEASE, 1)]);
}
