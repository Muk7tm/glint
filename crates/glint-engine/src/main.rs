// SPDX-License-Identifier: GPL-3.0-or-later
//! `glintd`: the Glint engine (PLAN §4.3).
#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    eprintln!("glintd {}: not implemented yet (Phase 2)", env!("CARGO_PKG_VERSION"));
    ExitCode::FAILURE
}
