// SPDX-License-Identifier: MIT
//! Monitors (PLAN §5.1). "Center of the screen" means the center of one monitor, never of the
//! whole root window, which on two monitors sits on the seam between them.
//!
//! The list comes from RandR 1.5 `GetMonitors`, falling back to RandR's CRTCs, then to the whole
//! root window. A monitor is chosen by its RandR name, or as the primary. RandR's primary can be a
//! disconnected output, leaving no active monitor primary; then the first monitor is used (C9).

use glint_core::pattern::Rect;
use x11rb::connection::Connection;
use x11rb::errors::{ConnectionError, ReplyError};
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::ConnectionExt as _;

use crate::caps::Version;

/// One monitor, in root-window pixels.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Monitor {
    pub name: String,
    pub rect: Rect,
    pub primary: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum MonitorError {
    #[error(transparent)]
    Connection(#[from] ConnectionError),
    #[error(transparent)]
    Reply(#[from] ReplyError),
}

/// The active monitors of the screen whose root is `root`, in the order RandR reports them.
pub fn list(
    conn: &impl Connection,
    root: u32,
    root_size: (u16, u16),
    randr: Option<Version>,
) -> Result<Vec<Monitor>, MonitorError> {
    let monitors = match randr {
        Some(v) if v >= Version::new(1, 5) => from_get_monitors(conn, root)?,
        Some(v) if v >= Version::new(1, 3) => from_crtcs(conn, root)?,
        _ => Vec::new(),
    };
    if monitors.is_empty() {
        return Ok(vec![Monitor {
            name: "screen".to_owned(),
            rect: Rect::new(0, 0, root_size.0, root_size.1),
            primary: true,
        }]);
    }
    Ok(monitors)
}

fn from_get_monitors(conn: &impl Connection, root: u32) -> Result<Vec<Monitor>, MonitorError> {
    let reply = conn.randr_get_monitors(root, true)?.reply()?;
    let mut out = Vec::with_capacity(reply.monitors.len());
    for m in reply.monitors.iter().filter(|m| m.width > 0 && m.height > 0) {
        let name = conn.get_atom_name(m.name)?.reply()?;
        out.push(Monitor {
            name: String::from_utf8_lossy(&name.name).into_owned(),
            rect: Rect::new(m.x.into(), m.y.into(), m.width, m.height),
            primary: m.primary,
        });
    }
    Ok(out)
}

fn from_crtcs(conn: &impl Connection, root: u32) -> Result<Vec<Monitor>, MonitorError> {
    let resources = conn.randr_get_screen_resources_current(root)?.reply()?;
    let primary = conn.randr_get_output_primary(root)?.reply()?.output;
    let mut out = Vec::new();
    for &crtc in &resources.crtcs {
        let info = conn.randr_get_crtc_info(crtc, resources.config_timestamp)?.reply()?;
        if info.mode == 0 || info.width == 0 || info.height == 0 || info.outputs.is_empty() {
            continue;
        }
        let output = info.outputs[0];
        let name = conn
            .randr_get_output_info(output, resources.config_timestamp)?
            .reply()?
            .name;
        out.push(Monitor {
            name: String::from_utf8_lossy(&name).into_owned(),
            rect: Rect::new(info.x.into(), info.y.into(), info.width, info.height),
            primary: info.outputs.contains(&primary),
        });
    }
    Ok(out)
}

/// The monitor a profile asks for: `"primary"` is the primary monitor, or the first one when no
/// active monitor is primary (C9); anything else is a RandR monitor name.
pub fn pick<'a>(monitors: &'a [Monitor], wanted: &str) -> Option<&'a Monitor> {
    if wanted == "primary" {
        monitors.iter().find(|m| m.primary).or_else(|| monitors.first())
    } else {
        monitors.iter().find(|m| m.name == wanted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(name: &str, x: i32, primary: bool) -> Monitor {
        Monitor {
            name: name.into(),
            rect: Rect::new(x, 0, 1920, 1080),
            primary,
        }
    }

    #[test]
    fn picking_a_monitor() {
        let two = [m("HDMI-1", 0, false), m("DP-1", 1920, true)];
        assert_eq!(pick(&two, "primary").unwrap().name, "DP-1");
        assert_eq!(pick(&two, "HDMI-1").unwrap().name, "HDMI-1");
        assert_eq!(pick(&two, "DP-2"), None);
        // This machine: the primary output is disconnected, so no active monitor is primary.
        let none_primary = [m("HDMI-1", 0, false)];
        assert_eq!(pick(&none_primary, "primary").unwrap().name, "HDMI-1");
        assert_eq!(pick(&[], "primary"), None);
    }
}
