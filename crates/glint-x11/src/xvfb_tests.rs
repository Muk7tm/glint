// SPDX-License-Identifier: MIT
//! Tests against a real X server (Xvfb). They're skipped, with a note, when Xvfb isn't installed.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, CreateWindowAux, WindowClass};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::caps::{self, Version};
use crate::conn::{self, ScreenInfo};
use crate::format::PixelFormat;
use crate::sandbox::{SandboxError, Xvfb};
use crate::wire::{self, RawConn};

/// An Xvfb, or `None` (and a note) if it isn't installed.
pub(crate) fn xvfb() -> Option<Xvfb> {
    match Xvfb::start(1280, 720) {
        Ok(x) => Some(x),
        Err(SandboxError::NotInstalled) => {
            eprintln!("skipped: Xvfb is not installed");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

/// Maps a window filled with `color` at `(x, y)` and waits until the server has drawn it.
pub(crate) fn colored_window(conn: &RustConnection, root: u32, x: i16, y: i16, w: u16, h: u16, color: u32) -> u32 {
    let win = conn.generate_id().unwrap();
    let aux = CreateWindowAux::new().background_pixel(color).override_redirect(1);
    conn.create_window(0, win, root, x, y, w, h, 0, WindowClass::INPUT_OUTPUT, 0, &aux)
        .unwrap();
    conn.map_window(win).unwrap();
    conn.sync().unwrap();
    win
}

#[test]
fn raw_connection_setup_and_the_root_format() {
    let Some(x) = xvfb() else { return };
    let (mut a, setup) = RawConn::connect(x.display()).unwrap();
    let screen = ScreenInfo::from_setup(&setup, 0).unwrap();
    assert_eq!((screen.width, screen.height, screen.root_depth), (1280, 720, 24));
    let fmt = PixelFormat::root(&setup, 0).unwrap();
    assert_eq!(fmt.stride(3), 12); // 32 bits per pixel
    a.sync().unwrap();
    assert_eq!(a.take_async_error(), (0, None));
}

#[test]
fn capabilities() {
    let Some(x) = xvfb() else { return };
    let c = conn::open(x.display()).unwrap();
    let caps = caps::probe(&c).unwrap();
    assert!(caps.xtest >= Version::new(2, 2), "{caps:?}");
    assert!(caps.xtest_opcode >= 128);
    assert!(caps.xinput.is_some_and(|v| v >= Version::new(2, 2)), "{caps:?}");
    assert!(
        caps.randr.is_some() && caps.shm.is_some() && caps.damage.is_some(),
        "{caps:?}"
    );
    assert!(!caps.xwayland);
}

#[test]
fn get_image_reads_a_window_through_the_root() {
    let Some(x) = xvfb() else { return };
    let c = conn::open(x.display()).unwrap();
    let root = c.setup().roots[0].root;
    colored_window(&c, root, 100, 50, 40, 30, 0x3B_D1_6F);
    let (mut a, setup) = RawConn::connect(x.display()).unwrap();
    let fmt = PixelFormat::root(&setup, 0).unwrap();
    // A 3×3 patch straddling the window's top-left corner: 4 pixels inside, 5 outside (black).
    let seq = a.send(&wire::get_image(root, 99, 49, 3, 3)).unwrap();
    let mut data = [0u8; 64];
    let len = a.wait_reply(seq, &mut data).unwrap();
    assert_eq!(len, fmt.image_len(3, 3));
    let mut px = [0u32; 9];
    fmt.decode(&data[..len], 3, 3, &mut px).unwrap();
    let g = 0x3B_D1_6F;
    assert_eq!(px, [0, 0, 0, 0, g, g, 0, g, g]);
    // Off the screen: BadMatch for this request, and the connection stays usable.
    let seq = a.send(&wire::get_image(root, 1279, 719, 3, 3)).unwrap();
    match a.wait_reply(seq, &mut data) {
        Err(wire::WireError::X(e)) => assert_eq!((e.code, e.major), (8, 73)),
        other => panic!("{other:?}"),
    }
    let seq = a.send(&wire::get_image(root, 110, 60, 1, 1)).unwrap();
    assert_eq!(a.wait_reply(seq, &mut data).unwrap(), 4);
}

#[test]
fn the_screen_backend_sees_a_color_appear_at_the_monitor_center() {
    use glint_core::color::{ColorModel, ColorRules, CompiledRules, Lut, ShadePreset};
    use glint_core::pattern::{PatternKind, SamplePattern};
    use glint_core::{Rgb, Verdict};

    use crate::capture::Capture;
    use crate::capture::getimage::GetImageCapture;
    use crate::monitors;

    let Some(x) = xvfb() else { return };
    let c = conn::open(x.display()).unwrap();
    let root = c.setup().roots[0].root;
    let caps = caps::probe(&c).unwrap();
    let list = monitors::list(&c, root, (1280, 720), caps.randr).unwrap();
    let monitor = monitors::pick(&list, "primary").unwrap();
    assert_eq!(
        monitor.rect,
        glint_core::pattern::Rect::new(0, 0, 1280, 720),
        "{list:?}"
    );

    let pattern = SamplePattern::from_profile(PatternKind::Square3, None, &[], None, None).unwrap();
    let rect = pattern.place(monitor.rect, (0, 0)).unwrap().rect;
    let rules = ColorRules {
        include: vec![ShadePreset::Normal.entry(ColorModel::Oklch, Rgb::new(0x3B, 0xD1, 0x6F))],
        ..ColorRules::default()
    };
    let lut = Lut::build(&CompiledRules::new(&rules));
    let (a, setup) = RawConn::connect(x.display()).unwrap();
    let mut capture = GetImageCapture::new(a, root, PixelFormat::root(&setup, 0).unwrap());
    let mut px = [0u32; 9];

    capture.grab(rect, &mut px).unwrap();
    assert_eq!(pattern.verdict(&lut, &px), Verdict::Absent);
    let win = colored_window(&c, root, 600, 330, 80, 60, 0x3B_D1_6F); // covers (640, 360)
    capture.grab(rect, &mut px).unwrap();
    assert_eq!(pattern.verdict(&lut, &px), Verdict::Enter);
    c.unmap_window(win).unwrap();
    c.sync().unwrap();
    capture.grab(rect, &mut px).unwrap();
    assert_eq!(pattern.verdict(&lut, &px), Verdict::Absent);
}

/// A window under the pointer with the input focus, recording the input it receives.
pub(crate) struct Receiver {
    conn: RustConnection,
    pub(crate) root: u32,
    win: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Got {
    ButtonPress(u8),
    ButtonRelease(u8),
    KeyPress(u8),
    KeyRelease(u8),
}

impl Receiver {
    pub(crate) fn open(display: crate::env::LocalDisplay) -> Self {
        use x11rb::protocol::xproto::{EventMask, InputFocus};
        let conn = conn::open(display).unwrap();
        let root = conn.setup().roots[0].root;
        let win = conn.generate_id().unwrap();
        let mask = EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::KEY_PRESS | EventMask::KEY_RELEASE;
        let aux = CreateWindowAux::new()
            .background_pixel(0x202020)
            .event_mask(mask)
            .override_redirect(1);
        conn.create_window(0, win, root, 0, 0, 400, 300, 0, WindowClass::INPUT_OUTPUT, 0, &aux)
            .unwrap();
        conn.map_window(win).unwrap();
        conn.warp_pointer(x11rb::NONE, win, 0, 0, 0, 0, 200, 150).unwrap();
        conn.set_input_focus(InputFocus::PARENT, win, x11rb::CURRENT_TIME)
            .unwrap();
        conn.sync().unwrap();
        Self { conn, root, win }
    }

    /// Every input event that arrives within `ms`.
    pub(crate) fn collect(&self, ms: u64) -> Vec<Got> {
        use x11rb::protocol::Event;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        let mut out = Vec::new();
        while std::time::Instant::now() < deadline {
            match self.conn.poll_for_event().unwrap() {
                Some(Event::ButtonPress(e)) if e.event == self.win => out.push(Got::ButtonPress(e.detail)),
                Some(Event::ButtonRelease(e)) if e.event == self.win => out.push(Got::ButtonRelease(e.detail)),
                Some(Event::KeyPress(e)) if e.event == self.win => out.push(Got::KeyPress(e.detail)),
                Some(Event::KeyRelease(e)) if e.event == self.win => out.push(Got::KeyRelease(e.detail)),
                Some(_) => {}
                None => std::thread::sleep(std::time::Duration::from_millis(1)),
            }
        }
        out
    }
}

fn injector(x: &Xvfb, action: crate::inject::Action) -> crate::inject::Injector {
    let c = conn::open(x.display()).unwrap();
    let caps = caps::probe(&c).unwrap();
    let (b, setup) = RawConn::connect(x.display()).unwrap();
    crate::inject::Injector::new(b, setup.roots[0].root, caps.xtest_opcode, action).unwrap()
}

#[test]
fn the_keymap_resolves_names_on_a_real_server() {
    use crate::keymap::{Keymap, keysym, keysym_from_name};
    let Some(x) = xvfb() else { return };
    let c = conn::open(x.display()).unwrap();
    let map = Keymap::fetch(&c).unwrap();
    let f7 = map.keycode(keysym_from_name("F7").unwrap()).unwrap();
    let e = map.keycode(keysym_from_name("e").unwrap()).unwrap();
    let shift = map.keycode(keysym::SHIFT_L).unwrap();
    assert!(f7 >= 8 && e >= 8 && f7 != e, "F7 {f7}, e {e}");
    let mods = map.modifier_keycodes();
    assert!(
        mods.contains(&shift) && mods.contains(&map.keycode(keysym::CONTROL_L).unwrap()),
        "{mods:?}"
    );
}

#[test]
fn a_click_is_one_press_then_one_release_after_the_hold() {
    use std::time::{Duration, Instant};

    use crate::inject::{Action, Input};
    let Some(x) = xvfb() else { return };
    let rx = Receiver::open(x.display());
    let mut inj = injector(&x, Action::new(Input::Button(1), Duration::from_millis(30)));
    let t0 = Instant::now();
    inj.press(t0).unwrap();
    inj.service(t0 + Duration::from_millis(10)).unwrap(); // hold not over yet
    assert!(inj.is_held());
    assert_eq!(rx.collect(50), [Got::ButtonPress(1)]);
    inj.service(t0 + Duration::from_millis(30)).unwrap();
    assert!(!inj.is_held());
    assert_eq!(rx.collect(50), [Got::ButtonRelease(1)]);
    inj.service(t0 + Duration::from_millis(60)).unwrap(); // nothing more
    assert_eq!(rx.collect(50), []);
}

#[test]
fn a_key_with_modifiers_and_a_press_while_held() {
    use std::time::{Duration, Instant};

    use crate::inject::{Action, Input};
    use crate::keymap::{Keymap, keysym};
    let Some(x) = xvfb() else { return };
    let rx = Receiver::open(x.display());
    let map = Keymap::fetch(&conn::open(x.display()).unwrap()).unwrap();
    let (shift, ctrl) = (
        map.keycode(keysym::SHIFT_L).unwrap(),
        map.keycode(keysym::CONTROL_L).unwrap(),
    );
    let action = Action::new(Input::Key(26), Duration::from_millis(5)).with_modifiers(&[shift, ctrl]);
    assert_eq!(action.modifiers(), [shift, ctrl]);
    let mut inj = injector(&x, action);
    let t0 = Instant::now();
    inj.press(t0).unwrap();
    inj.service(t0 + Duration::from_millis(5)).unwrap();
    let k = Got::KeyPress;
    let r = Got::KeyRelease;
    assert_eq!(rx.collect(50), [k(shift), k(ctrl), k(26), r(26), r(ctrl), r(shift)]);
    // Two presses with no service in between: the first is released before the second.
    inj.set_action(Action::new(Input::Button(3), Duration::from_millis(100)));
    inj.press(t0 + Duration::from_millis(10)).unwrap();
    inj.press(t0 + Duration::from_millis(11)).unwrap();
    inj.release_now().unwrap();
    assert_eq!(
        rx.collect(50),
        [
            Got::ButtonPress(3),
            Got::ButtonRelease(3),
            Got::ButtonPress(3),
            Got::ButtonRelease(3)
        ]
    );
}

#[test]
fn releasing_everything_from_either_connection() {
    use std::time::{Duration, Instant};

    use crate::inject::{Action, Input, release_everything_on};
    let Some(x) = xvfb() else { return };
    let rx = Receiver::open(x.display());
    let mut inj = injector(&x, Action::new(Input::Button(1), Duration::from_secs(10)));
    // Nothing held: releasing everything produces no events at all.
    inj.release_everything(&[26, 50]).unwrap();
    assert_eq!(rx.collect(50), []);
    // Held on B, released from another connection (the panic path's D).
    inj.press(Instant::now()).unwrap();
    assert_eq!(rx.collect(50), [Got::ButtonPress(1)]);
    let d = conn::open(x.display()).unwrap();
    release_everything_on(&d, rx.root, &[26]).unwrap();
    assert_eq!(rx.collect(50), [Got::ButtonRelease(1)]);
    // Held on B, released by B's own defensive release.
    inj.press(Instant::now()).unwrap();
    inj.release_everything(&[]).unwrap();
    assert_eq!(rx.collect(50), [Got::ButtonPress(1), Got::ButtonRelease(1)]);
    assert!(!inj.is_held());
}

#[test]
fn raw_key_events_and_the_xtest_filter() {
    use std::time::{Duration, Instant};

    use x11rb::protocol::xinput::ConnectionExt as _;
    use x11rb::protocol::xtest::ConnectionExt as _;

    use crate::hotkeys::{self, Matcher, ModifierKeys};
    use crate::keymap::{Keymap, keysym_from_name};

    let Some(x) = xvfb() else { return };
    let c = conn::open(x.display()).unwrap();
    caps::probe(&c).unwrap(); // announces XI 2.2
    let root = c.setup().roots[0].root;
    hotkeys::select_raw_keys(&c, root).unwrap();
    let xtest = hotkeys::xtest_devices(&c).unwrap();
    let devices = c.xinput_xi_query_device(0u16).unwrap().reply().unwrap();
    for d in &devices.infos {
        eprintln!(
            "device {} {:?} {:?}",
            d.deviceid,
            String::from_utf8_lossy(&d.name),
            d.type_
        );
    }
    assert!(!xtest.is_empty());

    let map = Keymap::fetch(&c).unwrap();
    let f8 = map.keycode(keysym_from_name("F8").unwrap()).unwrap();
    let injector = conn::open(x.display()).unwrap();
    injector.xtest_fake_input(2, f8, 0, root, 0, 0, 0).unwrap();
    injector.xtest_fake_input(3, f8, 0, root, 0, 0, 0).unwrap();
    injector.sync().unwrap();

    let mut keys = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline && keys.len() < 2 {
        match c.poll_for_event().unwrap() {
            Some(e) => keys.extend(hotkeys::raw_key(&e)),
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    assert_eq!(
        keys.iter().map(|k| (k.keycode, k.press)).collect::<Vec<_>>(),
        [(f8, true), (f8, false)]
    );
    assert!(keys.iter().all(|k| xtest.contains(&k.source)), "{keys:?} vs {xtest:?}");
    // F8 made by XTest is Glint-style input: the matcher ignores it.
    let (bindings, _) = hotkeys::bindings(&glint_core::settings::Hotkeys::default(), &map);
    let mut m = Matcher::new(xtest.clone(), ModifierKeys::new(&map));
    assert!(keys.iter().all(|&k| m.feed(k, &bindings).is_none()));

    // The same key "typed on the Xvfb keyboard" is a real hotkey.
    let keyboard = physical_keyboard(&c);
    physical_key(&injector, root, keyboard, f8, true);
    physical_key(&injector, root, keyboard, f8, false);
    let mut keys = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline && keys.len() < 2 {
        match c.poll_for_event().unwrap() {
            Some(e) => keys.extend(hotkeys::raw_key(&e)),
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    assert!(
        keys.len() == 2 && keys.iter().all(|k| k.source == u16::from(keyboard)),
        "{keys:?}"
    );
    assert_eq!(
        m.feed(keys[0], &bindings),
        Some((hotkeys::HotkeyAction::EmergencyStop, true))
    );
    assert_eq!(
        m.feed(keys[1], &bindings),
        Some((hotkeys::HotkeyAction::EmergencyStop, false))
    );
}

/// The id of Xvfb's own keyboard device, which isn't an XTEST device.
pub(crate) fn physical_keyboard(conn: &RustConnection) -> u8 {
    use x11rb::protocol::xinput::{ConnectionExt as _, DeviceType};
    let devices = conn.xinput_xi_query_device(0u16).unwrap().reply().unwrap();
    let d = devices
        .infos
        .iter()
        .find(|d| d.type_ == DeviceType::SLAVE_KEYBOARD && !String::from_utf8_lossy(&d.name).contains("XTEST"))
        .expect("a non-XTEST keyboard");
    u8::try_from(d.deviceid).unwrap()
}

/// A key press or release that comes from `device`, as if typed on it: XTest with an XInput
/// device-key event type and the device id, so the raw events' source is that device.
pub(crate) fn physical_key(conn: &RustConnection, root: u32, device: u8, keycode: u8, press: bool) {
    use x11rb::connection::RequestConnection as _;
    use x11rb::protocol::xinput;
    use x11rb::protocol::xtest::ConnectionExt as _;
    let base = conn
        .extension_information(xinput::X11_EXTENSION_NAME)
        .unwrap()
        .unwrap()
        .first_event;
    // XInput 1 event offsets: DeviceKeyPress 1, DeviceKeyRelease 2.
    let ty = base + if press { 1 } else { 2 };
    conn.xtest_fake_input(ty, keycode, 0, root, 0, 0, device).unwrap();
    conn.sync().unwrap();
}

#[test]
fn raw_button_events_name_their_device() {
    use std::time::{Duration, Instant};

    use x11rb::connection::RequestConnection as _;
    use x11rb::protocol::xinput::{self, ConnectionExt as _, DeviceType};
    use x11rb::protocol::xtest::ConnectionExt as _;

    use crate::hotkeys::{self, RawKey};

    let Some(x) = xvfb() else { return };
    let c = conn::open(x.display()).unwrap();
    caps::probe(&c).unwrap();
    let root = c.setup().roots[0].root;
    hotkeys::select_raw_keys(&c, root).unwrap();
    let xtest = hotkeys::xtest_devices(&c).unwrap();
    let devices = c.xinput_xi_query_device(0u16).unwrap().reply().unwrap();
    let mouse = devices
        .infos
        .iter()
        .find(|d| d.type_ == DeviceType::SLAVE_POINTER && !xtest.contains(&d.deviceid))
        .unwrap()
        .deviceid;

    let input = conn::open(x.display()).unwrap();
    // A button on the real mouse (Xvfb's has three: side buttons only on real hardware) ...
    let base = input
        .extension_information(xinput::X11_EXTENSION_NAME)
        .unwrap()
        .unwrap()
        .first_event;
    let device = u8::try_from(mouse).unwrap();
    input.xtest_fake_input(base + 3, 3, 0, root, 0, 0, device).unwrap();
    input.xtest_fake_input(base + 4, 3, 0, root, 0, 0, device).unwrap();
    // ... and button 8 from XTest itself, the way Glint's own Back clicks arrive.
    input.xtest_fake_input(4, 8, 0, root, 0, 0, 0).unwrap();
    input.xtest_fake_input(5, 8, 0, root, 0, 0, 0).unwrap();
    input.sync().unwrap();

    let mut got = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline && got.len() < 4 {
        match c.poll_for_event().unwrap() {
            Some(e) => got.extend(hotkeys::raw_key(&e)),
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    let raw = |keycode, press, source| RawKey {
        keycode,
        button: true,
        press,
        source,
        repeat: false,
    };
    assert_eq!(got[..2], [raw(3, true, mouse), raw(3, false, mouse)]);
    assert!(
        got[2].button && got[2].keycode == 8 && xtest.contains(&got[2].source),
        "{got:?}"
    );
}

#[test]
fn finding_and_describing_a_client_window() {
    use x11rb::protocol::xproto::{AtomEnum, CreateWindowAux, PropMode, WindowClass};
    use x11rb::wrapper::ConnectionExt as _;

    use crate::windows::{self, Atoms};

    let Some(x) = xvfb() else { return };
    let c = conn::open(x.display()).unwrap();
    let root = c.setup().roots[0].root;
    let atoms = Atoms::intern(&c).unwrap();
    // A window manager's frame at (100, 50) with the app's client inside, 4 px in and 20 px down.
    let frame = c.generate_id().unwrap();
    c.create_window(
        0,
        frame,
        root,
        100,
        50,
        308,
        224,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new(),
    )
    .unwrap();
    let client = c.generate_id().unwrap();
    c.create_window(
        0,
        client,
        frame,
        4,
        20,
        300,
        200,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new(),
    )
    .unwrap();
    c.change_property8(
        PropMode::REPLACE,
        client,
        AtomEnum::WM_CLASS,
        AtomEnum::STRING,
        b"sober\0org.vinegarhq.Sober\0",
    )
    .unwrap();
    c.change_property8(
        PropMode::REPLACE,
        client,
        atoms.net_wm_name,
        atoms.utf8_string,
        "Sober — Roblox".as_bytes(),
    )
    .unwrap();
    c.change_property32(PropMode::REPLACE, client, atoms.net_wm_pid, AtomEnum::CARDINAL, &[4242])
        .unwrap();
    c.change_property32(PropMode::REPLACE, client, atoms.wm_state, atoms.wm_state, &[1, 0])
        .unwrap();
    c.map_window(client).unwrap();
    c.map_window(frame).unwrap();
    c.sync().unwrap();

    assert_eq!(windows::client_of(&c, &atoms, frame), Some(client));
    assert_eq!(windows::frame_of(&c, root, client), frame);
    let info = windows::describe(&c, &atoms, root, client).unwrap();
    assert_eq!(
        (
            info.instance.as_str(),
            info.class.as_str(),
            info.title.as_str(),
            info.pid,
            info.frame
        ),
        ("sober", "org.vinegarhq.Sober", "Sober — Roblox", Some(4242), frame)
    );
    assert_eq!(
        windows::rect(&c, root, client).unwrap(),
        glint_core::pattern::Rect::new(104, 70, 300, 200)
    );
    // No _NET_CLIENT_LIST here (no window manager): found under the root's children.
    assert_eq!(
        windows::find(&c, &atoms, root, "org.vinegarhq.Sober", "").map(|i| i.id),
        Some(client)
    );
    assert_eq!(windows::find(&c, &atoms, root, "firefox", ""), None);
    assert_eq!(windows::active(&c, &atoms, root), None);
}

/// `screen-shm` (§5.2): reading through the MIT-SHM segment gives exactly the pixels the socket
/// gives, for the largest capture and a few small ones, at every edge of a two-color scene.
#[test]
fn shared_memory_reads_what_the_socket_reads() {
    use glint_core::pattern::Rect;

    use crate::capture::getimage::GetImageCapture;
    use crate::capture::shm::ShmSegment;
    use crate::capture::{Capture, MAX_PIXELS};

    let Some(x) = xvfb() else { return };
    let c = conn::open(x.display()).unwrap();
    let root = c.setup().roots[0].root;
    let _a = colored_window(&c, root, 100, 100, 200, 150, 0x3B_D1_6F);
    let _b = colored_window(&c, root, 250, 180, 120, 90, 0xE0_30_78);
    let (a, setup) = RawConn::connect(x.display()).unwrap();
    let fmt = PixelFormat::root(&setup, 0).unwrap();
    let segment = ShmSegment::new(&c, MAX_PIXELS * 4).unwrap();
    let mut shm = GetImageCapture::new(a, root, fmt).with_shm(segment);
    shm.set_shm(true);
    let (b, _) = RawConn::connect(x.display()).unwrap();
    let mut socket = GetImageCapture::new(b, root, fmt);
    let (mut via_shm, mut via_socket) = (vec![0u32; MAX_PIXELS], vec![0u32; MAX_PIXELS]);
    for rect in [
        Rect::new(220, 150, 63, 63),
        Rect::new(240, 170, 3, 3),
        Rect::new(0, 0, 1, 1),
        Rect::new(290, 230, 40, 17),
    ] {
        let n = usize::from(rect.w) * usize::from(rect.h);
        shm.grab(rect, &mut via_shm).unwrap();
        socket.grab(rect, &mut via_socket).unwrap();
        assert_eq!(via_shm[..n], via_socket[..n], "{rect:?}");
    }
    // Both colors and the background are in the big capture.
    let big = &via_shm[..63 * 63];
    for color in [0x3B_D1_6F, 0xE0_30_78] {
        assert!(big.contains(&color), "{color:06X}");
    }
}
