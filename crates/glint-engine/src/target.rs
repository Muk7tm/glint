// SPDX-License-Identifier: GPL-3.0-or-later
//! The picked window (PLAN §8): finding it by class, finding it again after it closes, its
//! rectangle, whether it's mapped, and whether it's in front.
//!
//! The controller owns a [`Tracker`] and feeds it the window events that connection C's event
//! thread forwards ([`WinEvent`]). The tracker answers with what changed; the controller decides
//! what that means for watching (restart, pause, stop).

use glint_core::pattern::Rect;
use glint_ipc::WindowState;
use glint_x11::windows::{self, Atoms, WindowInfo};
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt as _, EventMask, MapState, Window};
use x11rb::rust_connection::RustConnection;

/// The window events the tracker cares about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WinEvent {
    Configure(Window),
    Reparent(Window),
    Map(Window),
    Unmap(Window),
    Destroy(Window),
    Property(Window, u32),
    Focus(Window, bool),
}

impl WinEvent {
    /// The window event in `event`, if it is one.
    pub fn from_x(event: &Event) -> Option<Self> {
        Some(match event {
            Event::ConfigureNotify(e) => Self::Configure(e.window),
            Event::ReparentNotify(e) => Self::Reparent(e.window),
            Event::MapNotify(e) => Self::Map(e.window),
            Event::UnmapNotify(e) => Self::Unmap(e.window),
            Event::DestroyNotify(e) => Self::Destroy(e.window),
            Event::PropertyNotify(e) => Self::Property(e.window, e.atom),
            Event::FocusIn(e) => Self::Focus(e.event, true),
            Event::FocusOut(e) => Self::Focus(e.event, false),
            _ => return None,
        })
    }
}

/// What a window event changed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change {
    None,
    /// The window moved or was resized.
    Moved,
    /// Shown, hidden, in front or not: the status changed.
    Status,
    /// The window closed.
    Closed,
    /// The window manager's list of windows changed, or a window appeared: worth looking for a
    /// closed target again.
    Appeared,
}

pub struct Tracker {
    atoms: Option<Atoms>,
    root: Window,
    /// The window manager publishes `_NET_ACTIVE_WINDOW`; otherwise focus events stand in.
    ewmh: bool,
    window: Option<WindowInfo>,
    rect: Option<Rect>,
    mapped: bool,
    active: Option<Window>,
    focused: bool,
}

impl Tracker {
    /// Starts listening on the root window for the window list, the active window, and new
    /// top-level windows.
    pub fn new(conn: &RustConnection, root: Window) -> Self {
        let atoms = Atoms::intern(conn).ok();
        let mask = EventMask::PROPERTY_CHANGE | EventMask::SUBSTRUCTURE_NOTIFY;
        let _ = conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(mask));
        let _ = conn.flush();
        let mut t = Self {
            atoms,
            root,
            ewmh: false,
            window: None,
            rect: None,
            mapped: false,
            active: None,
            focused: false,
        };
        t.refresh_active(conn);
        t
    }

    pub fn window(&self) -> Option<&WindowInfo> {
        self.window.as_ref()
    }

    pub fn rect(&self) -> Option<Rect> {
        self.rect.filter(|_| self.mapped)
    }

    /// Whether the window is the active one (EWMH), or has the focus (without EWMH).
    pub fn in_front(&self) -> bool {
        let Some(w) = &self.window else { return false };
        self.mapped
            && if self.ewmh {
                self.active.is_some_and(|a| a == w.id || a == w.frame)
            } else {
                self.focused
            }
    }

    pub fn status(&self) -> Option<WindowState> {
        Some(match &self.window {
            None => return None,
            Some(_) if !self.mapped => WindowState::Hidden,
            Some(_) if self.in_front() => WindowState::InFront,
            Some(_) => WindowState::NotInFront,
        })
    }

    /// The client window under the top-level `top` (what a pick's click names), described.
    pub fn describe_top(&self, conn: &RustConnection, top: Window) -> Option<WindowInfo> {
        let atoms = self.atoms.as_ref()?;
        let client = windows::client_of(conn, atoms, top).unwrap_or(top);
        windows::describe(conn, atoms, self.root, client).ok()
    }

    /// Looks for the window `class` (and `title_contains`) names, and tracks it if found.
    pub fn acquire(&mut self, conn: &RustConnection, class: &str, title_contains: &str) -> bool {
        let found = self
            .atoms
            .as_ref()
            .and_then(|a| windows::find(conn, a, self.root, class, title_contains));
        match found {
            Some(info) => {
                self.track(conn, info);
                true
            }
            None => {
                self.forget();
                false
            }
        }
    }

    /// Tracks `info` from now on: its own events and its frame's.
    pub fn track(&mut self, conn: &RustConnection, info: WindowInfo) {
        let client_mask = EventMask::STRUCTURE_NOTIFY | EventMask::FOCUS_CHANGE;
        let _ = conn.change_window_attributes(info.id, &ChangeWindowAttributesAux::new().event_mask(client_mask));
        if info.frame != info.id {
            let frame_mask = EventMask::STRUCTURE_NOTIFY;
            let _ = conn.change_window_attributes(info.frame, &ChangeWindowAttributesAux::new().event_mask(frame_mask));
        }
        let _ = conn.flush();
        self.mapped = conn
            .get_window_attributes(info.id)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|a| a.map_state == MapState::VIEWABLE);
        self.focused = conn
            .get_input_focus()
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|f| f.focus == info.id || f.focus == info.frame);
        self.rect = windows::rect(conn, self.root, info.id).ok();
        self.window = Some(info);
        self.refresh_active(conn);
    }

    pub fn forget(&mut self) {
        self.window = None;
        self.rect = None;
        self.mapped = false;
    }

    fn refresh_active(&mut self, conn: &RustConnection) {
        if let Some(atoms) = &self.atoms {
            self.ewmh = windows::has_ewmh_active(conn, atoms, self.root);
            self.active = windows::active(conn, atoms, self.root);
        }
    }

    /// Updates the tracker for `event` and says what changed.
    pub fn event(&mut self, conn: &RustConnection, event: WinEvent) -> Change {
        let Some(atoms) = self.atoms else { return Change::None };
        let (client, frame) = match &self.window {
            Some(w) => (w.id, w.frame),
            None => (x11rb::NONE, x11rb::NONE),
        };
        let ours = |w: Window| self.window.is_some() && (w == client || w == frame);
        match event {
            WinEvent::Property(w, atom) if w == self.root && atom == atoms.net_active_window => {
                let was = self.in_front();
                self.refresh_active(conn);
                if was == self.in_front() {
                    Change::None
                } else {
                    Change::Status
                }
            }
            WinEvent::Property(w, atom) if w == self.root && atom == atoms.net_client_list => Change::Appeared,
            WinEvent::Map(w) if ours(w) => {
                self.mapped = true;
                self.rect = windows::rect(conn, self.root, client).ok();
                Change::Status
            }
            WinEvent::Map(_) => Change::Appeared,
            WinEvent::Unmap(w) if w == client => {
                self.mapped = false;
                Change::Status
            }
            WinEvent::Destroy(w) if w == client => {
                self.forget();
                Change::Closed
            }
            WinEvent::Configure(w) | WinEvent::Reparent(w) if ours(w) => {
                if let WinEvent::Reparent(_) = event
                    && let Some(win) = &mut self.window
                {
                    win.frame = windows::frame_of(conn, self.root, client);
                    let frame_mask = EventMask::STRUCTURE_NOTIFY;
                    let _ = conn
                        .change_window_attributes(win.frame, &ChangeWindowAttributesAux::new().event_mask(frame_mask));
                }
                let rect = windows::rect(conn, self.root, client).ok();
                if rect == self.rect {
                    Change::None
                } else {
                    self.rect = rect;
                    Change::Moved
                }
            }
            WinEvent::Focus(w, focused) if !self.ewmh && w == client => {
                let was = self.in_front();
                self.focused = focused;
                if was == self.in_front() {
                    Change::None
                } else {
                    Change::Status
                }
            }
            _ => Change::None,
        }
    }
}

/// The window-buffer self-check (§5.2): reads the window's center from its own image and from
/// the screen, 20 times over 200 ms. A blank window image (the app draws into a child window),
/// or one that disagrees with the screen on more than 10% of the reads while the window is in
/// front, means reading the screen instead. Returns the window's pixel format when it passes.
pub fn window_buffer_check(
    conn: &RustConnection,
    root: Window,
    root_format: &glint_x11::format::PixelFormat,
    win: Window,
    rect: Rect,
    in_front: bool,
) -> Result<glint_x11::format::PixelFormat, String> {
    use glint_x11::format::PixelFormat;
    use x11rb::protocol::xproto::ImageFormat;
    let attrs = conn
        .get_window_attributes(win)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    let depth = conn
        .get_geometry(win)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?
        .depth;
    let format = PixelFormat::for_visual(conn.setup(), attrs.visual, depth).map_err(|e| e.to_string())?;
    let (cx, cy) = ((rect.w / 2) as i16, (rect.h / 2) as i16);
    let (sx, sy) = ((rect.x + i32::from(cx)) as i16, (rect.y + i32::from(cy)) as i16);
    let read = |drawable: Window, x: i16, y: i16, f: &PixelFormat| -> Option<u32> {
        let r = conn
            .get_image(ImageFormat::Z_PIXMAP, drawable, x, y, 1, 1, u32::MAX)
            .ok()?
            .reply()
            .ok()?;
        let mut px = [0u32; 1];
        f.decode(&r.data, 1, 1, &mut px).ok()?;
        Some(px[0])
    };
    let (mut own, mut screen) = ([0u32; CHECKS], [0u32; CHECKS]);
    for i in 0..CHECKS {
        own[i] = read(win, cx, cy, &format).ok_or("its own image can't be read")?;
        screen[i] = read(root, sx, sy, root_format).ok_or("the screen can't be read")?;
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    agree(&own, &screen, in_front)?;
    Ok(format)
}

/// Reads per self-check.
const CHECKS: usize = 20;

/// The self-check's verdict on the window's own reads and the screen's at the same spot.
fn agree(own: &[u32], screen: &[u32], in_front: bool) -> Result<(), String> {
    if own.iter().all(|&p| p == 0) && screen.iter().any(|&p| p != 0) {
        return Err("its own image is blank (it draws into a child window), so Glint reads the screen instead".into());
    }
    let differ = own.iter().zip(screen).filter(|(a, b)| a != b).count();
    // More than 10% of the reads.
    if in_front && differ * 10 > own.len() {
        return Err("its own image doesn't match the screen, so Glint reads the screen instead".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use x11rb::protocol::xproto::{CreateWindowAux, WindowClass};
    use x11rb::wrapper::ConnectionExt as _;

    #[test]
    fn the_self_check_s_verdict() {
        let green = [0x3B_D1_6F; 20];
        assert!(agree(&green, &green, true).is_ok());
        // Blank while the screen shows something: the app draws into a child window.
        assert!(agree(&[0; 20], &green, true).unwrap_err().contains("blank"));
        // Black on both is just a black spot.
        assert!(agree(&[0; 20], &[0; 20], true).is_ok());
        // Two reads in twenty can catch a frame in between; three can't.
        let mut own = green;
        own[..2].fill(0x10_10_10);
        assert!(agree(&own, &green, true).is_ok());
        own[2] = 0x10_10_10;
        assert!(agree(&own, &green, true).unwrap_err().contains("doesn't match"));
        // Behind another window, the screen shows that window: no comparison.
        assert!(agree(&own, &green, false).is_ok());
    }

    #[test]
    fn the_self_check_reads_a_plain_window() {
        let x = match glint_x11::sandbox::Xvfb::start(640, 480) {
            Ok(x) => x,
            Err(glint_x11::sandbox::SandboxError::NotInstalled) => return,
            Err(e) => panic!("{e}"),
        };
        let c = glint_x11::conn::open(x.display()).unwrap();
        let screen = &c.setup().roots[0];
        let (root, depth, visual) = (screen.root, screen.root_depth, screen.root_visual);
        let root_format = glint_x11::format::PixelFormat::root(c.setup(), 0).unwrap();
        let win = c.generate_id().unwrap();
        c.create_window(
            depth,
            win,
            root,
            100,
            50,
            200,
            100,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux::new().background_pixel(0x3B_D1_6F),
        )
        .unwrap();
        c.map_window(win).unwrap();
        c.sync().unwrap();
        let format = window_buffer_check(&c, root, &root_format, win, Rect::new(100, 50, 200, 100), true).unwrap();
        assert_eq!(format, root_format);
    }
}
