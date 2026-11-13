// SPDX-License-Identifier: MIT
//! Starting the engine when none answers (§4.3): `glintd` next to this program, else the one on
//! `$PATH`. It runs on its own, not as part of the window, so closing the window with *Keep
//! running* leaves it watching.

use std::io;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

/// The engine program to start.
pub fn engine_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .map(|p| p.with_file_name("glintd"))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("glintd"))
}

/// Starts the engine and returns it, so the caller can notice if it exits at once.
pub fn start_engine() -> io::Result<Child> {
    Command::new(engine_path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
}

/// Waits for `child` on a thread of its own, so it never lingers as a zombie while the window is
/// open; if the window closes first, the engine simply keeps running.
pub fn reap(mut child: Child) {
    let _ = std::thread::Builder::new()
        .name("glint-engine-wait".into())
        .spawn(move || {
            let _ = child.wait();
        });
}
