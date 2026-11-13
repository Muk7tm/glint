// SPDX-License-Identifier: GPL-3.0-or-later
//! `glintd`: the Glint engine (PLAN §13). Scaffold only; implemented in Phase 2.
#![forbid(unsafe_code)]

mod controller;
mod hot;
mod lifecycle;
mod lock;
mod rt;
mod store;
mod telemetry;

use std::process::ExitCode;

fn main() -> ExitCode {
    eprintln!("glintd {}: not implemented yet (Phase 2)", env!("CARGO_PKG_VERSION"));
    ExitCode::FAILURE
}
