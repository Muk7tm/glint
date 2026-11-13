// SPDX-License-Identifier: GPL-3.0-or-later
//! Every scenario of §16 that runs without real hardware (Phase 3), against the real `glintd` on
//! Xvfb. The counts are kept small so every `cargo test` runs them; `glint-lab` runs longer ones.
//! The tests check correctness only, never timing (§16): shared CI runners are far too noisy.
#![allow(clippy::unwrap_used)]

use std::path::Path;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use glint_lab::{Options, Report};

/// Sandboxes running at once. Each is an Xvfb plus an engine checking 2,000 times a second; a
/// crowded machine adds blind time to every run, which shows up as reported possible misses.
/// (`cargo nextest` runs each test in its own process; `.config/nextest.toml` limits those.)
static RUNNING: Mutex<usize> = Mutex::new(0);
static FREED: Condvar = Condvar::new();

struct Slot;

impl Slot {
    fn take() -> Self {
        let most = std::thread::available_parallelism().map_or(2, |n| (n.get() / 4).clamp(2, 4));
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        while *running >= most {
            running = FREED.wait(running).unwrap_or_else(|e| e.into_inner());
        }
        *running += 1;
        Slot
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        *RUNNING.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        FREED.notify_one();
    }
}

fn run(scenario: &str, count: u32) -> Option<Report> {
    let _slot = Slot::take();
    let opts = Options {
        count: Some(count),
        seed: 42,
        duration: Duration::from_secs(8),
        ..Options::default()
    };
    match glint_lab::run_in_sandbox(Path::new(env!("CARGO_BIN_EXE_glintd")), scenario, &opts) {
        Ok(Some(report)) => {
            eprint!("{}", report.summary());
            assert!(report.passed(), "{}", report.summary());
            Some(report)
        }
        Ok(None) => {
            eprintln!("skipped: Xvfb is not installed");
            None
        }
        Err(e) => panic!("{scenario}: {e}"),
    }
}

/// The run had appearances, and each one that called for a click got one, or the engine
/// explained why not.
fn every_appearance_clicked(r: &Report) {
    assert!(r.appearances > 0, "{}", r.summary());
    assert!(
        r.clicks + r.reported + r.unseen >= r.appearances - r.optional,
        "{}",
        r.summary()
    );
}

#[test]
fn basic() {
    if let Some(r) = run("basic", 15) {
        every_appearance_clicked(&r);
        assert!(r.split.is_some(), "{}", r.summary());
    }
}

#[test]
fn shades() {
    if let Some(r) = run("shades", 12) {
        every_appearance_clicked(&r);
    }
}

#[test]
fn near_miss() {
    if let Some(r) = run("near-miss", 16) {
        assert_eq!((r.appearances, r.clicks), (0, 0));
    }
}

#[test]
fn distractor() {
    // Every fifth flash is a real appearance.
    if let Some(r) = run("distractor", 15) {
        assert_eq!(r.appearances, 3, "{}", r.summary());
    }
}

#[test]
fn flicker() {
    if let Some(r) = run("flicker", 10) {
        every_appearance_clicked(&r);
    }
}

#[test]
fn boundary_noise() {
    if let Some(r) = run("boundary-noise", 10) {
        every_appearance_clicked(&r);
    }
}

#[test]
fn fade() {
    if let Some(r) = run("fade", 5) {
        every_appearance_clicked(&r);
    }
}

#[test]
fn rapid() {
    // Per round: five appearances, then five merged into one (more only if the lab overslept a
    // short gap past Gone after, which the judge then counts as a real gap).
    if let Some(r) = run("rapid", 2) {
        assert!(r.appearances - r.optional >= 12, "{}", r.summary());
        assert!(r.appearances <= 20, "{}", r.summary());
    }
}

#[test]
fn arm_while_present() {
    if let Some(r) = run("arm-while-present", 3) {
        assert_eq!(r.appearances, 3, "{}", r.summary());
    }
}

#[test]
fn live_edit() {
    // Per round: the blue after an edit, the blue again, then the green.
    if let Some(r) = run("live-edit", 2) {
        assert_eq!(r.appearances, 6, "{}", r.summary());
    }
}

#[test]
fn stall() {
    run("stall", 12);
}

#[test]
fn two_engines() {
    run("two-engines", 1);
}

#[test]
fn feedback() {
    if let Some(r) = run("feedback", 2) {
        assert_eq!(r.self_stops.len(), 2, "{}", r.summary());
    }
}

#[test]
fn action_matrix() {
    if let Some(r) = run("action-matrix", 1) {
        assert_eq!(r.clicks, r.appearances, "{}", r.summary());
    }
}

#[test]
fn kill_mid_press() {
    run("kill-mid-press", 2);
}

#[test]
fn action_equals_hotkey() {
    run("action-equals-hotkey", 1);
}

#[test]
fn soak() {
    if let Some(r) = run("soak", 1) {
        assert!(r.appearances > 5, "{}", r.summary());
    }
}
