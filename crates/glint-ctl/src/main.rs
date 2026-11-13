// SPDX-License-Identifier: GPL-3.0-or-later
//! `glint-ctl`: the Glint engine from the command line (PLAN §10).
#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::Parser as _;

fn main() -> ExitCode {
    glint_ctl::run(&glint_ctl::Cli::parse())
}
