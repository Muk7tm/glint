// SPDX-License-Identifier: GPL-3.0-or-later
//! Picking a color with the mouse (§10 `pick_color`, §12.6 magnifier). The engine grabs the
//! pointer with a crosshair on connection C, streams the pixels around it, and returns the one
//! clicked. Escape or any other button cancels. The UI only shows the frames; it never grabs or
//! injects anything.

use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{ConnectionExt as _, EventMask, GrabMode, GrabStatus, ImageFormat};
use x11rb::rust_connection::RustConnection;

use glint_x11::format::PixelFormat;

/// The magnifier shows `SIZE` × `SIZE` screen pixels.
pub const SIZE: u16 = 11;

/// What the pointer and keyboard grab delivers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickInput {
    Moved(i16, i16),
    Clicked(u8, i16, i16),
    Key(u8),
}

/// The crosshair from X's cursor font (`XC_crosshair`).
pub fn crosshair(conn: &RustConnection) -> Result<u32, String> {
    let e = |e: &dyn std::fmt::Display| e.to_string();
    let font = conn.generate_id().map_err(|x| e(&x))?;
    conn.open_font(font, b"cursor").map_err(|x| e(&x))?;
    let cursor = conn.generate_id().map_err(|x| e(&x))?;
    conn.create_glyph_cursor(cursor, font, font, 34, 35, 0, 0, 0, 0xffff, 0xffff, 0xffff)
        .map_err(|x| e(&x))?;
    conn.close_font(font).map_err(|x| e(&x))?;
    Ok(cursor)
}

/// Grabs the pointer (with `cursor`) and the keyboard for a pick.
pub fn grab(conn: &RustConnection, root: u32, cursor: u32) -> Result<(), String> {
    let mask = EventMask::POINTER_MOTION | EventMask::BUTTON_PRESS;
    let pointer = conn
        .grab_pointer(
            false,
            root,
            mask,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
            x11rb::NONE,
            cursor,
            x11rb::CURRENT_TIME,
        )
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    if pointer.status != GrabStatus::SUCCESS {
        return Err("Another program is holding the mouse; try again.".into());
    }
    // Without the keyboard, Escape doesn't cancel, but a right click still does.
    let _ = conn
        .grab_keyboard(false, root, x11rb::CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)
        .map(|c| c.reply());
    Ok(())
}

pub fn ungrab(conn: &RustConnection) {
    let _ = conn.ungrab_pointer(x11rb::CURRENT_TIME);
    let _ = conn.ungrab_keyboard(x11rb::CURRENT_TIME);
    let _ = conn.flush();
}

/// Where the pointer is now, in root coordinates.
pub fn pointer(conn: &RustConnection, root: u32) -> Option<(i16, i16)> {
    let p = conn.query_pointer(root).ok()?.reply().ok()?;
    Some((p.root_x, p.root_y))
}

/// The `SIZE` × `SIZE` pixels around `at` (moved inside the screen near its edges), row by row,
/// and the one at `at`.
pub fn around(
    conn: &RustConnection,
    root: u32,
    format: &PixelFormat,
    screen: (u16, u16),
    at: (i16, i16),
) -> Result<(Vec<u32>, u32), String> {
    let half = (SIZE / 2) as i32;
    let clamp = |v: i16, len: u16| (i32::from(v) - half).clamp(0, (i32::from(len) - i32::from(SIZE)).max(0));
    let (x0, y0) = (clamp(at.0, screen.0), clamp(at.1, screen.1));
    let reply = conn
        .get_image(ImageFormat::Z_PIXMAP, root, x0 as i16, y0 as i16, SIZE, SIZE, u32::MAX)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    let mut px = vec![0u32; usize::from(SIZE) * usize::from(SIZE)];
    format
        .decode(&reply.data, SIZE, SIZE, &mut px)
        .map_err(|e| e.to_string())?;
    let cx = (i32::from(at.0) - x0).clamp(0, i32::from(SIZE) - 1) as usize;
    let cy = (i32::from(at.1) - y0).clamp(0, i32::from(SIZE) - 1) as usize;
    let center = px[cy * usize::from(SIZE) + cx];
    Ok((px, center))
}
