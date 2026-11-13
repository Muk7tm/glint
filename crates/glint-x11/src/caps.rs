// SPDX-License-Identifier: MIT
//! Which X extensions the server offers (PLAN §4.3 step 3). XTEST is required; everything else
//! degrades gracefully.

use std::fmt;

use x11rb::connection::Connection;
use x11rb::errors::{ConnectionError, ReplyError};
use x11rb::protocol::composite::ConnectionExt as _;
use x11rb::protocol::damage::ConnectionExt as _;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::screensaver::ConnectionExt as _;
use x11rb::protocol::shm::ConnectionExt as _;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xinput::ConnectionExt as _;
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::protocol::{composite, damage, randr, screensaver, shm, xfixes, xinput, xtest};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
}

impl Version {
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// The server's extensions, probed once at startup on connection C.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Caps {
    pub xtest: Version,
    /// XTEST's major opcode, which the raw input connection needs.
    pub xtest_opcode: u8,
    pub shm: Option<Version>,
    pub damage: Option<Version>,
    pub composite: Option<Version>,
    pub randr: Option<Version>,
    /// XInput 2, announced as 2.2 (raw events reach us even while a game grabs the keyboard, §9).
    pub xinput: Option<Version>,
    pub xfixes: Option<Version>,
    pub screensaver: Option<Version>,
    /// The XWAYLAND extension: this is XWayland, so only X11 apps can be watched (§8).
    pub xwayland: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CapsError {
    #[error("the X server has no XTEST extension, which Glint needs to click")]
    NoXtest,
    #[error("the X server has XTEST {0}; Glint needs 2.2")]
    OldXtest(Version),
    #[error(transparent)]
    Connection(#[from] ConnectionError),
    #[error(transparent)]
    Reply(#[from] ReplyError),
}

/// Probes every extension Glint can use.
pub fn probe(conn: &impl Connection) -> Result<Caps, CapsError> {
    let has = |name: &'static str| -> Result<bool, ConnectionError> { Ok(conn.extension_information(name)?.is_some()) };
    let xtest_info = conn
        .extension_information(xtest::X11_EXTENSION_NAME)?
        .ok_or(CapsError::NoXtest)?;
    let v = conn.xtest_get_version(2, 2)?.reply()?;
    let xtest = Version::new(v.major_version.into(), v.minor_version.into());
    if xtest < Version::new(2, 2) {
        return Err(CapsError::OldXtest(xtest));
    }
    let shm = if has(shm::X11_EXTENSION_NAME)? {
        let r = conn.shm_query_version()?.reply()?;
        Some(Version::new(r.major_version.into(), r.minor_version.into()))
    } else {
        None
    };
    let damage = if has(damage::X11_EXTENSION_NAME)? {
        let r = conn.damage_query_version(1, 1)?.reply()?;
        Some(Version::new(r.major_version, r.minor_version))
    } else {
        None
    };
    let composite = if has(composite::X11_EXTENSION_NAME)? {
        let r = conn.composite_query_version(0, 4)?.reply()?;
        Some(Version::new(r.major_version, r.minor_version))
    } else {
        None
    };
    let randr = if has(randr::X11_EXTENSION_NAME)? {
        let r = conn.randr_query_version(1, 6)?.reply()?;
        Some(Version::new(r.major_version, r.minor_version))
    } else {
        None
    };
    let xinput = if has(xinput::X11_EXTENSION_NAME)? {
        let r = conn.xinput_xi_query_version(2, 2)?.reply()?;
        Some(Version::new(r.major_version.into(), r.minor_version.into()))
    } else {
        None
    };
    let xfixes = if has(xfixes::X11_EXTENSION_NAME)? {
        let r = conn.xfixes_query_version(6, 0)?.reply()?;
        Some(Version::new(r.major_version, r.minor_version))
    } else {
        None
    };
    let screensaver = if has(screensaver::X11_EXTENSION_NAME)? {
        let r = conn.screensaver_query_version(1, 1)?.reply()?;
        Some(Version::new(
            r.server_major_version.into(),
            r.server_minor_version.into(),
        ))
    } else {
        None
    };
    Ok(Caps {
        xtest,
        xtest_opcode: xtest_info.major_opcode,
        shm,
        damage,
        composite,
        randr,
        xinput,
        xfixes,
        screensaver,
        xwayland: has("XWAYLAND")?,
    })
}
