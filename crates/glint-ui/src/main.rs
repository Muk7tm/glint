// SPDX-License-Identifier: GPL-3.0-or-later
//! `glint`: the Glint window (Slint) (PLAN §13). Scaffold only; implemented in Phase 4.
#![forbid(unsafe_code)]

mod bind;
mod client;
mod spawn;

use std::process::ExitCode;

fn main() -> ExitCode {
    eprintln!("glint {}: not implemented yet (Phase 4)", env!("CARGO_PKG_VERSION"));
    ExitCode::FAILURE
}
