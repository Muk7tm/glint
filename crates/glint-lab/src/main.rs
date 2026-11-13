// SPDX-License-Identifier: GPL-3.0-or-later
//! `glint-lab`: the Glint test lab (PLAN §13). Scaffold only; implemented in Phase 3.
#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    eprintln!("glint-lab {}: not implemented yet (Phase 3)", env!("CARGO_PKG_VERSION"));
    ExitCode::FAILURE
}
