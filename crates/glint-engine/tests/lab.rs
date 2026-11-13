// SPDX-License-Identifier: MIT
//! Every scenario of §16 that runs without real hardware (Phase 3), against the real `glintd` on
//! Xvfb. The counts are kept small so every `cargo test` runs them; `glint-lab` runs longer ones.
//! The tests check correctness only, never timing (§16): shared CI runners are far too noisy.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::Path;
use std::time::Duration;

use common::Slot;
use glint_core::profile::Backend;
use glint_lab::{Options, Report};

fn run(scenario: &str, count: u32) -> Option<Report> {
    run_with(scenario, count, Backend::Auto)
}

fn run_with(scenario: &str, count: u32, backend: Backend) -> Option<Report> {
    let _slot = Slot::take();
    let opts = Options {
        count: Some(count),
        backend,
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

#[test]
fn moving_the_window() {
    // Per round: the first appearance, and the one after the window is back home.
    if let Some(r) = run("move", 3) {
        assert_eq!(r.appearances - r.optional, 6, "{}", r.summary());
    }
}

#[test]
fn focus_changes() {
    if let Some(r) = run("focus", 3) {
        assert_eq!(r.appearances - r.optional, 6, "{}", r.summary());
    }
}

#[test]
fn ui_overlap() {
    if let Some(r) = run("ui-overlap", 2) {
        assert_eq!(r.appearances, 2, "{}", r.summary());
    }
}

#[test]
fn moving_the_window_read_from_its_own_image() {
    if let Some(r) = run_with("move", 2, Backend::WindowBuffer) {
        assert!(r.title.contains("backend window-buffer"), "{}", r.summary());
        assert_eq!(r.appearances - r.optional, 4, "{}", r.summary());
    }
}

#[test]
fn focus_changes_read_from_the_window_s_own_image() {
    if let Some(r) = run_with("focus", 2, Backend::WindowBuffer) {
        assert!(r.title.contains("backend window-buffer"), "{}", r.summary());
        assert_eq!(r.appearances - r.optional, 4, "{}", r.summary());
    }
}
