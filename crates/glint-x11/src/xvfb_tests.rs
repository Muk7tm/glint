// SPDX-License-Identifier: GPL-3.0-or-later
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
