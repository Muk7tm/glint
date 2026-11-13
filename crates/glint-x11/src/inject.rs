// SPDX-License-Identifier: MIT
//! XTest input (PLAN §6, Appendix A.4).
//!
//! The [`Injector`] owns the raw connection B (ADR 0003). A press goes out the moment the trigger
//! fires; the release follows `hold` later, timed by [`glint_core::Hold`]. Modifiers are pressed
//! before the button or key and released after it, all in one write. The connection is impervious
//! to other clients' server grabs, so a window manager's grab can't stall a click.
//!
//! Releasing something that isn't held does nothing (measured, MEASUREMENTS.md §5), which makes
//! "release everything" safe on every stop path, at startup, and from another connection.

use std::time::{Duration, Instant};

use glint_core::Hold;
use x11rb::connection::Connection;
use x11rb::errors::ConnectionError;
use x11rb::protocol::xtest::ConnectionExt as _;

use crate::wire::{self, FAKE_INPUT_LEN, RawConn, WireError, XError};

pub const KEY_PRESS: u8 = 2;
pub const KEY_RELEASE: u8 = 3;
pub const BUTTON_PRESS: u8 = 4;
pub const BUTTON_RELEASE: u8 = 5;

/// Every mouse button Glint can press: left, middle, right, back, forward (§6).
pub const BUTTONS: [u8; 5] = [1, 2, 3, 8, 9];

/// The most modifiers one action can hold.
pub const MAX_MODIFIERS: usize = 4;

/// How long the input connection may go without reading what the server sent it.
const DRAIN_EVERY: Duration = Duration::from_millis(50);

/// A mouse button or a key, by X number.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Input {
    Button(u8),
    Key(u8),
}

impl Input {
    fn press(self) -> (u8, u8) {
        match self {
            Self::Button(b) => (BUTTON_PRESS, b),
            Self::Key(k) => (KEY_PRESS, k),
        }
    }

    fn release(self) -> (u8, u8) {
        match self {
            Self::Button(b) => (BUTTON_RELEASE, b),
            Self::Key(k) => (KEY_RELEASE, k),
        }
    }
}

/// One click, resolved to X numbers: what to press, with which modifier keys, for how long, and
/// optionally where to move the pointer first (*Click at: Watched spot*).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Action {
    pub input: Input,
    modifiers: [u8; MAX_MODIFIERS],
    n_modifiers: u8,
    pub hold: Duration,
    pub warp_to: Option<(i16, i16)>,
}

impl Action {
    pub fn new(input: Input, hold: Duration) -> Self {
        Self {
            input,
            modifiers: [0; MAX_MODIFIERS],
            n_modifiers: 0,
            hold,
            warp_to: None,
        }
    }

    /// Adds modifier keycodes, pressed in this order and released in reverse. At most
    /// [`MAX_MODIFIERS`]; extras are ignored.
    pub fn with_modifiers(mut self, keycodes: &[u8]) -> Self {
        let n = keycodes.len().min(MAX_MODIFIERS);
        self.modifiers[..n].copy_from_slice(&keycodes[..n]);
        self.n_modifiers = n as u8;
        self
    }

    pub fn with_warp(mut self, to: Option<(i16, i16)>) -> Self {
        self.warp_to = to;
        self
    }

    pub fn modifiers(&self) -> &[u8] {
        &self.modifiers[..usize::from(self.n_modifiers)]
    }
}

/// Room for a stale release, a warp and a press, each with every modifier.
const BATCH: usize = 2 * (MAX_MODIFIERS + 1) * FAKE_INPUT_LEN + 24;

pub struct Injector {
    conn: RawConn,
    root: u32,
    xtest: u8,
    action: Action,
    timer: Hold,
    /// What is held right now, released even if the action has changed since.
    pressed: Option<Action>,
    last_drain: Option<Instant>,
    out: [u8; BATCH],
}

impl Injector {
    /// Takes over connection B and makes it impervious to server grabs (§6).
    pub fn new(mut conn: RawConn, root: u32, xtest_opcode: u8, action: Action) -> Result<Self, WireError> {
        conn.send(&wire::grab_control(xtest_opcode, true))?;
        conn.sync()?;
        if let (_, Some(e)) = conn.take_async_error() {
            return Err(WireError::X(e));
        }
        Ok(Self {
            conn,
            root,
            xtest: xtest_opcode,
            action,
            timer: Hold::default(),
            pressed: None,
            last_drain: None,
            out: [0; BATCH],
        })
    }

    /// Uses `action` from the next press on; whatever is held now is still released correctly.
    pub fn set_action(&mut self, action: Action) {
        self.action = action;
    }

    pub fn is_held(&self) -> bool {
        self.pressed.is_some()
    }

    /// Presses the action now. If the previous press is somehow still held (the hot loop releases
    /// a due hold before each check, and the trigger's minimum gap is longer than the hold), it is
    /// released first, so presses and releases always alternate (invariant 6).
    pub fn press(&mut self, now: Instant) -> Result<(), WireError> {
        let (mut len, mut count) = (0, 0);
        self.timer.press(now, self.action.hold);
        if let Some(held) = self.pressed.take() {
            (len, count) = self.append_release(held, len, count);
        }
        let a = self.action;
        if let Some((x, y)) = a.warp_to {
            self.out[len..len + 24].copy_from_slice(&wire::warp_pointer(self.root, x, y));
            (len, count) = (len + 24, count + 1);
        }
        for &m in a.modifiers() {
            (len, count) = self.append(KEY_PRESS, m, len, count);
        }
        let (ty, detail) = a.input.press();
        (len, count) = self.append(ty, detail, len, count);
        self.conn.send_batch(&self.out[..len], count)?;
        self.pressed = Some(a);
        Ok(())
    }

    /// Releases the held input once its hold is over, and now and then reads what the server sent
    /// on this connection. Called on every iteration, before the check is decided.
    pub fn service(&mut self, now: Instant) -> Result<(), WireError> {
        if self.timer.due(now) {
            self.send_release()?;
        }
        if self
            .last_drain
            .is_none_or(|t| now.saturating_duration_since(t) >= DRAIN_EVERY)
        {
            self.last_drain = Some(now);
            self.conn.drain()?;
        }
        Ok(())
    }

    /// Releases the held input at once: every stop path.
    pub fn release_now(&mut self) -> Result<(), WireError> {
        self.timer.release();
        self.send_release()
    }

    /// Releases every button Glint can press and the given keycodes, held or not: the defensive
    /// release at startup and `release_all` (C10). Waits until the server has processed them.
    pub fn release_everything(&mut self, keycodes: &[u8]) -> Result<(), WireError> {
        self.timer.release();
        self.pressed = None;
        for &b in &BUTTONS {
            self.conn
                .send(&wire::fake_input(self.xtest, BUTTON_RELEASE, b, self.root))?;
        }
        for &k in keycodes {
            self.conn
                .send(&wire::fake_input(self.xtest, KEY_RELEASE, k, self.root))?;
        }
        self.conn.sync()
    }

    /// The latest X error for a request on this connection, if there was one since the last call.
    pub fn take_error(&mut self) -> Option<XError> {
        self.conn.take_async_error().1
    }

    fn send_release(&mut self) -> Result<(), WireError> {
        let Some(held) = self.pressed.take() else { return Ok(()) };
        let (len, count) = self.append_release(held, 0, 0);
        self.conn.send_batch(&self.out[..len], count)?;
        Ok(())
    }

    fn append_release(&mut self, held: Action, mut len: usize, mut count: u16) -> (usize, u16) {
        let (ty, detail) = held.input.release();
        (len, count) = self.append(ty, detail, len, count);
        for &m in held.modifiers().iter().rev() {
            (len, count) = self.append(KEY_RELEASE, m, len, count);
        }
        (len, count)
    }

    fn append(&mut self, ty: u8, detail: u8, len: usize, count: u16) -> (usize, u16) {
        self.out[len..len + FAKE_INPUT_LEN].copy_from_slice(&wire::fake_input(self.xtest, ty, detail, self.root));
        (len + FAKE_INPUT_LEN, count + 1)
    }
}

/// The same release as [`Injector::release_everything`] on an x11rb connection: the panic path
/// (connection D) and `glint-ctl release-all` when no engine answers (C10).
pub fn release_everything_on(conn: &impl Connection, root: u32, keycodes: &[u8]) -> Result<(), ConnectionError> {
    for &b in &BUTTONS {
        conn.xtest_fake_input(BUTTON_RELEASE, b, x11rb::CURRENT_TIME, root, 0, 0, 0)?;
    }
    for &k in keycodes {
        conn.xtest_fake_input(KEY_RELEASE, k, x11rb::CURRENT_TIME, root, 0, 0, 0)?;
    }
    conn.flush()
}
