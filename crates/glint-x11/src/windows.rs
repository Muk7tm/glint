// SPDX-License-Identifier: MIT
//! Window identity, tracking and focus (PLAN §8): the application's client window (the one
//! carrying `WM_STATE`, not the window manager's frame), its class, title and PID, its rectangle
//! in root coordinates, the windows the window manager lists, and which one is active.

use glint_core::pattern::Rect;
use x11rb::connection::Connection;
use x11rb::errors::{ConnectionError, ReplyError};
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, Window};

/// How deep under a top-level window the client is looked for (frame → decoration → client).
const DEPTH: usize = 4;

#[derive(Debug, thiserror::Error)]
pub enum WindowError {
    #[error(transparent)]
    Connection(#[from] ConnectionError),
    #[error(transparent)]
    Reply(#[from] ReplyError),
}

/// The atoms window lookup needs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Atoms {
    pub wm_state: u32,
    pub net_wm_name: u32,
    pub utf8_string: u32,
    pub net_wm_pid: u32,
    pub net_client_list: u32,
    pub net_active_window: u32,
}

impl Atoms {
    pub fn intern(conn: &impl Connection) -> Result<Self, WindowError> {
        let names: [&[u8]; 6] = [
            b"WM_STATE",
            b"_NET_WM_NAME",
            b"UTF8_STRING",
            b"_NET_WM_PID",
            b"_NET_CLIENT_LIST",
            b"_NET_ACTIVE_WINDOW",
        ];
        let cookies = names
            .iter()
            .map(|n| conn.intern_atom(false, n))
            .collect::<Result<Vec<_>, _>>()?;
        let mut atoms = [0u32; 6];
        for (slot, cookie) in atoms.iter_mut().zip(cookies) {
            *slot = cookie.reply()?.atom;
        }
        let [
            wm_state,
            net_wm_name,
            utf8_string,
            net_wm_pid,
            net_client_list,
            net_active_window,
        ] = atoms;
        Ok(Self {
            wm_state,
            net_wm_name,
            utf8_string,
            net_wm_pid,
            net_client_list,
            net_active_window,
        })
    }
}

/// What Glint knows about an application window.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct WindowInfo {
    /// The client window.
    pub id: Window,
    /// Its top-level ancestor: the window manager's frame, or the client itself.
    pub frame: Window,
    /// `WM_CLASS`: instance and class.
    pub instance: String,
    pub class: String,
    /// `_NET_WM_NAME`, or `WM_NAME`.
    pub title: String,
    pub pid: Option<u32>,
}

impl WindowInfo {
    /// Whether this window is the one `class` (and `title_contains`, when not empty) names.
    pub fn matches(&self, class: &str, title_contains: &str) -> bool {
        let class_ok =
            class.is_empty() || self.class.eq_ignore_ascii_case(class) || self.instance.eq_ignore_ascii_case(class);
        let title_ok = title_contains.is_empty() || self.title.to_lowercase().contains(&title_contains.to_lowercase());
        class_ok && title_ok && !(class.is_empty() && title_contains.is_empty())
    }
}

fn property(conn: &impl Connection, win: Window, atom: u32, kind: u32) -> Result<Vec<u8>, WindowError> {
    Ok(conn.get_property(false, win, atom, kind, 0, 1024)?.reply()?.value)
}

fn has_wm_state(conn: &impl Connection, atoms: &Atoms, win: Window) -> bool {
    conn.get_property(false, win, atoms.wm_state, AtomEnum::ANY, 0, 0)
        .ok()
        .and_then(|c| c.reply().ok())
        .is_some_and(|r| r.type_ != u32::from(AtomEnum::NONE))
}

/// The client window under `top` (`top` itself if it carries `WM_STATE`), breadth first.
pub fn client_of(conn: &impl Connection, atoms: &Atoms, top: Window) -> Option<Window> {
    let mut level = vec![top];
    for _ in 0..DEPTH {
        if let Some(&w) = level.iter().find(|&&w| has_wm_state(conn, atoms, w)) {
            return Some(w);
        }
        let mut next = Vec::new();
        for w in level {
            if let Ok(tree) = conn.query_tree(w).map(|c| c.reply()) {
                next.extend(tree.into_iter().flat_map(|t| t.children));
            }
        }
        if next.is_empty() {
            return None;
        }
        level = next;
    }
    None
}

/// The top-level ancestor of `win` (the child of `root` it lives under).
pub fn frame_of(conn: &impl Connection, root: Window, win: Window) -> Window {
    let mut w = win;
    for _ in 0..16 {
        match conn.query_tree(w).ok().and_then(|c| c.reply().ok()) {
            Some(t) if t.parent != root && t.parent != x11rb::NONE => w = t.parent,
            _ => break,
        }
    }
    w
}

/// The client's class, title, PID and frame.
pub fn describe(conn: &impl Connection, atoms: &Atoms, root: Window, win: Window) -> Result<WindowInfo, WindowError> {
    let class = property(conn, win, AtomEnum::WM_CLASS.into(), AtomEnum::STRING.into())?;
    let mut parts = class
        .split(|&b| b == 0)
        .map(|p| String::from_utf8_lossy(p).into_owned());
    let instance = parts.next().unwrap_or_default();
    let class = parts.next().unwrap_or_default();
    let mut title = String::from_utf8_lossy(&property(conn, win, atoms.net_wm_name, atoms.utf8_string)?).into_owned();
    if title.is_empty() {
        title =
            String::from_utf8_lossy(&property(conn, win, AtomEnum::WM_NAME.into(), AtomEnum::ANY.into())?).into_owned();
    }
    let pid = property(conn, win, atoms.net_wm_pid, AtomEnum::CARDINAL.into())?
        .get(..4)
        .map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]));
    Ok(WindowInfo {
        id: win,
        frame: frame_of(conn, root, win),
        instance,
        class,
        title,
        pid,
    })
}

/// The application windows: the window manager's `_NET_CLIENT_LIST`, or the clients under the
/// root's mapped children when there is none.
pub fn clients(conn: &impl Connection, atoms: &Atoms, root: Window) -> Vec<Window> {
    let listed: Vec<Window> = property(conn, root, atoms.net_client_list, AtomEnum::WINDOW.into())
        .unwrap_or_default()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&b| u32::from_ne_bytes(b))
        .collect();
    if !listed.is_empty() {
        return listed;
    }
    let children = conn
        .query_tree(root)
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|t| t.children)
        .unwrap_or_default();
    children.into_iter().filter_map(|w| client_of(conn, atoms, w)).collect()
}

/// The first application window that `class` and `title_contains` name.
pub fn find(
    conn: &impl Connection,
    atoms: &Atoms,
    root: Window,
    class: &str,
    title_contains: &str,
) -> Option<WindowInfo> {
    clients(conn, atoms, root)
        .into_iter()
        .filter_map(|w| describe(conn, atoms, root, w).ok())
        .find(|i| i.matches(class, title_contains))
}

/// The window's rectangle in root coordinates.
pub fn rect(conn: &impl Connection, root: Window, win: Window) -> Result<Rect, WindowError> {
    let g = conn.get_geometry(win)?.reply()?;
    let t = conn.translate_coordinates(win, root, 0, 0)?.reply()?;
    Ok(Rect::new(i32::from(t.dst_x), i32::from(t.dst_y), g.width, g.height))
}

/// The active window, from the window manager's `_NET_ACTIVE_WINDOW` (EWMH).
pub fn active(conn: &impl Connection, atoms: &Atoms, root: Window) -> Option<Window> {
    property(conn, root, atoms.net_active_window, AtomEnum::WINDOW.into())
        .ok()?
        .get(..4)
        .map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
        .filter(|&w| w != x11rb::NONE)
}

/// Whether the window manager publishes `_NET_ACTIVE_WINDOW` (EWMH); without it, focus events
/// on the client stand in.
pub fn has_ewmh_active(conn: &impl Connection, atoms: &Atoms, root: Window) -> bool {
    conn.get_property(false, root, atoms.net_active_window, AtomEnum::ANY, 0, 0)
        .ok()
        .and_then(|c| c.reply().ok())
        .is_some_and(|r| r.type_ != u32::from(AtomEnum::NONE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_by_class_and_title() {
        let w = WindowInfo {
            instance: "sober".into(),
            class: "org.vinegarhq.Sober".into(),
            title: "Sober".into(),
            ..WindowInfo::default()
        };
        assert!(w.matches("org.vinegarhq.Sober", ""));
        assert!(w.matches("SOBER", ""));
        assert!(w.matches("org.vinegarhq.Sober", "sob"));
        assert!(!w.matches("org.vinegarhq.Sober", "Roblox"));
        assert!(!w.matches("firefox", ""));
        assert!(w.matches("", "Sober"));
        // Nothing to go by matches nothing.
        assert!(!w.matches("", ""));
    }
}
