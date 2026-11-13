// SPDX-License-Identifier: GPL-3.0-or-later
//! X11 backends for the engine (PLAN §5.1–§5.2, §6, §8, §9).
//!
//! `unsafe` is allowed only in `capture/shm.rs` (PLAN §13), with a SAFETY
//! comment on every block; everything else stays safe.
#![deny(unsafe_code)]

pub mod caps;
pub mod capture;
pub mod conn;
pub mod env;
pub mod format;
pub mod hotkeys;
pub mod inject;
pub mod monitors;
pub mod picker;
#[cfg(any(test, feature = "sandbox"))]
pub mod sandbox;
pub mod windows;
pub mod wire;

#[cfg(test)]
mod xvfb_tests;
