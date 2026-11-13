// SPDX-License-Identifier: GPL-3.0-or-later
//! `glint-ctl`: the Glint command-line client (PLAN §13). Scaffold only; implemented in Phase 2.
#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    eprintln!("glint-ctl {}: not implemented yet (Phase 2)", env!("CARGO_PKG_VERSION"));
    ExitCode::FAILURE
}
