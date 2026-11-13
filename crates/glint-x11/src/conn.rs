// SPDX-License-Identifier: GPL-3.0-or-later
//! Opening Glint's X connections (PLAN §4.1). A (capture) and B (input) use the raw path in
//! [`crate::wire`] (ADR 0003); C (the controller: windows, focus, hotkeys, picking, monitors) and
//! D (the idle emergency-release connection) use x11rb.

use x11rb::errors::ConnectError;
use x11rb::rust_connection::RustConnection;
use x11rb_protocol::protocol::xproto::Setup;

use crate::env::LocalDisplay;

/// Opens an x11rb connection to `display` (connections C and D).
pub fn open(display: LocalDisplay) -> Result<RustConnection, ConnectError> {
    RustConnection::connect(Some(&display.to_string())).map(|(conn, _)| conn)
}

/// What the engine needs to know about its screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScreenInfo {
    pub root: u32,
    pub width: u16,
    pub height: u16,
    pub root_visual: u32,
    pub root_depth: u8,
}

impl ScreenInfo {
    pub fn from_setup(setup: &Setup, screen: u16) -> Option<Self> {
        let s = setup.roots.get(usize::from(screen))?;
        Some(Self {
            root: s.root,
            width: s.width_in_pixels,
            height: s.height_in_pixels,
            root_visual: s.root_visual,
            root_depth: s.root_depth,
        })
    }
}
