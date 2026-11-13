// SPDX-License-Identifier: GPL-3.0-or-later
//! The desktop around the engine (PLAN §8): whether a compositor runs (it owns the selection
//! `_NET_WM_CM_S<screen>`, followed live through XFixes), how fast the monitors refresh, and
//! whether the screen saver or locker is on (MIT-SCREEN-SAVER).

use glint_x11::caps::Caps;
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::screensaver::{self, ConnectionExt as _};
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

/// What changed on the desktop.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DesktopEvent {
    /// A compositor started (`true`) or went away.
    Compositor(bool),
    /// The screen saver or locker came on (`true`) or went off.
    Locked(bool),
}

/// The compositor selection's atom, once watching started.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Watch {
    cm: Option<u32>,
}

impl Watch {
    /// Starts following the compositor selection and the screen saver on `root`.
    pub fn start(conn: &RustConnection, root: u32, screen: u16, caps: &Caps) -> Self {
        let name = format!("_NET_WM_CM_S{screen}");
        let cm = conn
            .intern_atom(false, name.as_bytes())
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.atom);
        if let (Some(atom), Some(_)) = (cm, caps.xfixes) {
            let mask = xfixes::SelectionEventMask::SET_SELECTION_OWNER
                | xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
                | xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE;
            let _ = conn.xfixes_select_selection_input(root, atom, mask);
        }
        if caps.screensaver.is_some() {
            let _ = conn.screensaver_select_input(root, screensaver::Event::NOTIFY_MASK);
        }
        let _ = conn.flush();
        Self { cm }
    }

    /// The desktop event in `event`, if it is one.
    pub fn event(&self, event: &Event) -> Option<DesktopEvent> {
        match event {
            Event::XfixesSelectionNotify(e) if Some(e.selection) == self.cm => {
                Some(DesktopEvent::Compositor(e.owner != x11rb::NONE))
            }
            Event::ScreensaverNotify(e) => Some(DesktopEvent::Locked(e.state != screensaver::State::OFF)),
            _ => None,
        }
    }
}

/// The fastest active monitor's refresh rate, from RandR's modes.
pub fn max_refresh(conn: &RustConnection, root: u32) -> Option<f64> {
    let res = conn.randr_get_screen_resources_current(root).ok()?.reply().ok()?;
    let mut best: Option<f64> = None;
    for &crtc in &res.crtcs {
        let Some(info) = conn
            .randr_get_crtc_info(crtc, res.config_timestamp)
            .ok()
            .and_then(|c| c.reply().ok())
        else {
            continue;
        };
        if info.mode == 0 {
            continue;
        }
        if let Some(m) = res.modes.iter().find(|m| m.id == info.mode) {
            let dots = f64::from(m.htotal) * f64::from(m.vtotal);
            if dots > 0.0 && m.dot_clock > 0 {
                let hz = f64::from(m.dot_clock) / dots;
                best = Some(best.map_or(hz, |b| b.max(hz)));
            }
        }
    }
    best
}
