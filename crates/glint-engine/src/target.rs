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
