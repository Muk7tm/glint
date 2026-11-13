// SPDX-License-Identifier: GPL-3.0-or-later
//! `glintd`: the Glint engine (PLAN §4.3).
#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::Parser as _;
use glint_engine::lifecycle::{Options, run};

fn main() -> ExitCode {
    match run(Options::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("glintd: {e}");
            ExitCode::from(e.exit_code())
        }
    }
}
