// SPDX-License-Identifier: GPL-3.0-or-later
//! Phase 2's exit scenarios (§19): `basic`, `near-miss` and `arm-while-present` against the real
//! `glintd` on Xvfb. The counts are kept small so every `cargo test` runs them; `glint-lab` runs
//! longer ones.

use std::path::Path;

fn run(scenario: &str, count: u32) {
    match glint_lab::run_in_sandbox(Path::new(env!("CARGO_BIN_EXE_glintd")), scenario, count, 42) {
        Ok(Some(report)) => {
            eprint!("{}", report.summary());
            assert!(report.passed(), "{}", report.summary());
            if scenario == "basic" {
                assert_eq!((report.appearances, report.clicks), (count, count));
            }
        }
        Ok(None) => eprintln!("skipped: Xvfb is not installed"),
        Err(e) => panic!("{scenario}: {e}"),
    }
}

#[test]
fn basic() {
    run("basic", 20);
}

#[test]
fn near_miss() {
    run("near-miss", 16);
}

#[test]
fn arm_while_present() {
    run("arm-while-present", 3);
}
