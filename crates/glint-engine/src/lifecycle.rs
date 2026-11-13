// SPDX-License-Identifier: GPL-3.0-or-later
//! Startup and the stop paths that can't go through the hot loop (PLAN §4.3, §6): the panic hook
//! and the signals.
//!
//! Connection D is opened at startup and stays idle; its only job is the emergency release. A
//! release of something that isn't held does nothing, so the emergency path releases every button
//! Glint can press and every key it might hold, without knowing what is held.

use std::sync::Arc;

use arc_swap::ArcSwap;
use glint_x11::inject::release_everything_on;
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
use x11rb::errors::ConnectionError;
use x11rb::rust_connection::RustConnection;

/// Connection D and what to release on it.
pub struct Emergency {
    conn: RustConnection,
    root: u32,
    /// Every modifier keycode and every key a profile uses; the controller keeps it current.
    keycodes: ArcSwap<Vec<u8>>,
}

impl Emergency {
    pub fn new(conn: RustConnection, root: u32, keycodes: Vec<u8>) -> Self {
        Self {
            conn,
            root,
            keycodes: ArcSwap::from_pointee(keycodes),
        }
    }

    pub fn set_keycodes(&self, keycodes: Vec<u8>) {
        self.keycodes.store(Arc::new(keycodes));
    }

    /// Releases every button Glint can press and every key it might hold.
    pub fn release_everything(&self) -> Result<(), ConnectionError> {
        release_everything_on(&self.conn, self.root, &self.keycodes.load())
    }
}

/// Releases input on connection D before the usual panic report. Release builds use
/// `panic = "abort"`, so the process ends right after; nothing else runs first (§4.3).
pub fn install_panic_hook(emergency: Arc<Emergency>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = emergency.release_everything();
        previous(info);
    }));
}

/// SIGTERM, SIGINT and SIGHUP mean a graceful stop: `on_signal` runs on a thread of its own for
/// each one (it should release input, clean up and exit 0, §4.3).
pub fn watch_signals(on_signal: impl Fn(i32) + Send + 'static) -> std::io::Result<()> {
    let mut signals = signal_hook::iterator::Signals::new([SIGTERM, SIGINT, SIGHUP])?;
    std::thread::Builder::new()
        .name("glintd-signals".into())
        .spawn(move || {
            for signal in signals.forever() {
                on_signal(signal);
            }
        })?;
    Ok(())
}
