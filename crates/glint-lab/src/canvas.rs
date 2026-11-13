// SPDX-License-Identifier: MIT
//! The lab's window (§16): it covers a monitor, shows colors at the watched spot, and records
//! the input the engine sends into it.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use glint_core::pattern::Rect;
use glint_x11::env::LocalDisplay;
use glint_x11::keymap::Keymap;
use x11rb::connection::Connection;
use x11rb::protocol::Event as XEvent;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeGCAux, ConfigureWindowAux, ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask, Gravity,
    InputFocus, PropMode, Rectangle, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::{LabError, x};

/// The lab's target color: Appendix B's green.
pub const GREEN: u32 = 0x3B_D1_6F;
/// The canvas's background.
pub const BACKGROUND: u32 = 0x10_10_10;
/// The color patch around the watched spot: larger than any built-in pattern.
pub const PATCH: u16 = 40;
/// The square around the watched spot that [`Canvas::clear`] repaints; distractors stay inside it.
pub const STAGE: u16 = 120;

/// A rectangle in root coordinates: x, y, width, height.
pub type Area = (i32, i32, u16, u16);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InputKind {
    ButtonPress(u8),
    ButtonRelease(u8),
    KeyPress(u8),
    KeyRelease(u8),
}

impl InputKind {
    pub fn is_press(self) -> bool {
        matches!(self, Self::ButtonPress(_) | Self::KeyPress(_))
    }
}

/// One input event on the canvas: when the lab read it, and the server's timestamp (ms).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Input {
    pub at: Instant,
    pub server_ms: u32,
    pub kind: InputKind,
}

/// When a change reached the screen, as closely as a client can tell: its request went out
/// after `sent`, and the server had carried it out by `drawn` (its round trip had come back).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Drawn {
    pub sent: Instant,
    pub drawn: Instant,
}

/// A window covering a monitor, with the watched spot at the monitor's anchor pixel.
pub struct Canvas {
    conn: Arc<RustConnection>,
    root: u32,
    win: u32,
    gc: u32,
    /// The window's top-left corner, in root coordinates.
    origin: (i32, i32),
    /// The monitor's anchor pixel (§5.1), in root coordinates.
    anchor: (i32, i32),
    inputs: Receiver<Input>,
}

impl Canvas {
    /// Opens the canvas over `area` (in root coordinates), focuses it, and parks the pointer
    /// inside it away from the watched spot, so the engine's clicks and keys land on it.
    pub fn open(display: LocalDisplay, area: Rect) -> Result<Self, LabError> {
        Self::create(display, area, None)
    }

    /// A game-like window at `area` for window mode (§8): managed, not override-redirect, with
    /// `WM_CLASS` `class`, the title `class`, and `WM_STATE` (the sandbox has no window manager
    /// to set it). It keeps its pixels when resized, so a resize moves the center over them.
    pub fn game(display: LocalDisplay, area: Rect, class: &str) -> Result<Self, LabError> {
        Self::create(display, area, Some(class))
    }

    fn create(display: LocalDisplay, area: Rect, class: Option<&str>) -> Result<Self, LabError> {
        let conn = Arc::new(glint_x11::conn::open(display).map_err(x)?);
        let root = conn.setup().roots[usize::from(display.screen)].root;
        let win = conn.generate_id().map_err(x)?;
        let aux = CreateWindowAux::new()
            .background_pixel(BACKGROUND)
            .event_mask(
                EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::KEY_PRESS | EventMask::KEY_RELEASE,
            )
            .bit_gravity(Gravity::NORTH_WEST)
            .override_redirect(u32::from(class.is_none()));
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
        if let Some(class) = class {
            let atom = |name: &[u8]| -> Result<u32, LabError> {
                Ok(conn.intern_atom(false, name).map_err(x)?.reply().map_err(x)?.atom)
            };
            let wm_class = format!("{class}\0{class}\0");
            conn.change_property8(
                PropMode::REPLACE,
                win,
                AtomEnum::WM_CLASS,
                AtomEnum::STRING,
                wm_class.as_bytes(),
            )
            .map_err(x)?;
            conn.change_property8(
                PropMode::REPLACE,
                win,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                class.as_bytes(),
            )
            .map_err(x)?;
            let wm_state = atom(b"WM_STATE")?;
            conn.change_property32(PropMode::REPLACE, win, wm_state, wm_state, &[1, 0])
                .map_err(x)?;
        }
        conn.map_window(win).map_err(x)?;
        conn.warp_pointer(x11rb::NONE, win, 0, 0, 0, 0, 20, 20).map_err(x)?;
        conn.set_input_focus(InputFocus::PARENT, win, x11rb::CURRENT_TIME)
            .map_err(x)?;
        let gc = conn.generate_id().map_err(x)?;
        conn.create_gc(gc, win, &CreateGCAux::new().foreground(BACKGROUND))
            .map_err(x)?;
        conn.sync().map_err(x)?;
        let (tx, inputs) = mpsc::channel();
        let reader = Arc::clone(&conn);
        std::thread::Builder::new()
            .name("lab-events".into())
            .spawn(move || {
                while let Ok(event) = reader.wait_for_event() {
                    let at = Instant::now();
                    let (server_ms, kind) = match event {
                        XEvent::ButtonPress(e) if e.event == win => (e.time, InputKind::ButtonPress(e.detail)),
                        XEvent::ButtonRelease(e) if e.event == win => (e.time, InputKind::ButtonRelease(e.detail)),
                        XEvent::KeyPress(e) if e.event == win => (e.time, InputKind::KeyPress(e.detail)),
                        XEvent::KeyRelease(e) if e.event == win => (e.time, InputKind::KeyRelease(e.detail)),
                        _ => continue,
                    };
                    if tx.send(Input { at, server_ms, kind }).is_err() {
                        break;
                    }
                }
            })
            .map_err(x)?;
        Ok(Self {
            conn,
            root,
            win,
            gc,
            origin: (area.x, area.y),
            anchor: area.anchor(),
            inputs,
        })
    }

    /// The watched spot: the monitor's (or window's) anchor pixel, in root coordinates.
    pub fn anchor(&self) -> (i32, i32) {
        self.anchor
    }

    /// Moves or resizes the window, and puts the pointer back inside it.
    pub fn set_rect(&mut self, area: Rect) -> Result<Drawn, LabError> {
        let aux = ConfigureWindowAux::new()
            .x(area.x)
            .y(area.y)
            .width(u32::from(area.w))
            .height(u32::from(area.h));
        let sent = Instant::now();
        self.conn.configure_window(self.win, &aux).map_err(x)?;
        self.conn
            .warp_pointer(x11rb::NONE, self.win, 0, 0, 0, 0, 20, 20)
            .map_err(x)?;
        self.conn.sync().map_err(x)?;
        self.origin = (area.x, area.y);
        self.anchor = area.anchor();
        Ok(Drawn {
            sent,
            drawn: Instant::now(),
        })
    }

    /// Gives this window the keyboard focus (what *Only click in this window* follows without a
    /// window manager).
    pub fn focus(&self) -> Result<Drawn, LabError> {
        let sent = Instant::now();
        self.conn
            .set_input_focus(InputFocus::PARENT, self.win, x11rb::CURRENT_TIME)
            .map_err(x)?;
        self.conn.sync().map_err(x)?;
        Ok(Drawn {
            sent,
            drawn: Instant::now(),
        })
    }

    /// Puts the pointer inside this window, so clicks land on it.
    pub fn warp_inside(&self) -> Result<(), LabError> {
        self.conn
            .warp_pointer(x11rb::NONE, self.win, 0, 0, 0, 0, 20, 20)
            .map_err(x)?;
        self.conn.sync().map_err(x)
    }

    /// Fills `areas` (root coordinates) with `color` in one request, which the server carries out
    /// whole: a check sees all of it or none of it.
    pub fn fill(&self, color: u32, areas: &[Area]) -> Result<Drawn, LabError> {
        let rects = areas
            .iter()
            .map(|&(ax, ay, w, h)| {
                Ok(Rectangle {
                    x: i16::try_from(ax - self.origin.0).map_err(x)?,
                    y: i16::try_from(ay - self.origin.1).map_err(x)?,
                    width: w,
                    height: h,
                })
            })
            .collect::<Result<Vec<_>, LabError>>()?;
        self.conn
            .change_gc(self.gc, &ChangeGCAux::new().foreground(color))
            .map_err(x)?;
        let sent = Instant::now();
        self.conn.poly_fill_rectangle(self.win, self.gc, &rects).map_err(x)?;
        self.conn.sync().map_err(x)?;
        Ok(Drawn {
            sent,
            drawn: Instant::now(),
        })
    }

    /// A square of `side` pixels centered on the watched spot.
    pub fn square(&self, side: u16) -> Area {
        let half = i32::from(side / 2);
        (self.anchor.0 - half, self.anchor.1 - half, side, side)
    }

    /// Fills the patch around the watched spot with `color`.
    pub fn show(&self, color: u32) -> Result<Drawn, LabError> {
        self.fill(color, &[self.square(PATCH)])
    }

    /// Repaints the whole stage with the background.
    pub fn clear(&self) -> Result<Drawn, LabError> {
        self.fill(BACKGROUND, &[self.square(STAGE)])
    }

    /// Every input received since the last call.
    pub fn take_inputs(&self) -> Vec<Input> {
        self.inputs.try_iter().collect()
    }

    /// The next input, waiting at most `timeout`.
    pub fn next_input(&self, timeout: Duration) -> Option<Input> {
        self.inputs.recv_timeout(timeout).ok()
    }

    /// The pointer's button and modifier state (core `KeyButMask`).
    pub fn pointer_state(&self) -> Result<u16, LabError> {
        let reply = self.conn.query_pointer(self.root).map_err(x)?.reply().map_err(x)?;
        Ok(u16::from(reply.mask))
    }

    /// Whether `keycode` is down, from the server's keymap bit vector.
    pub fn key_down(&self, keycode: u8) -> Result<bool, LabError> {
        let keys = self.conn.query_keymap().map_err(x)?.reply().map_err(x)?.keys;
        Ok(keys[usize::from(keycode / 8)] & (1 << (keycode % 8)) != 0)
    }

    /// The server's current keymap.
    pub fn keymap(&self) -> Result<Keymap, LabError> {
        Keymap::fetch(self.conn.as_ref()).map_err(x)
    }
}
