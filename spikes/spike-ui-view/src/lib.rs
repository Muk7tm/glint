// SPDX-License-Identifier: GPL-3.0-or-later
//! The Slint-generated view for `spike-ui`.
//!
//! Slint 1.18.1's generated code expands `ItemTreeVTable_static!`, which carries
//! `#[allow(unsafe_code)]`; that is a hard error under `#![forbid(unsafe_code)]`
//! (E0453). So the generated code lives here under `deny`, and the hand-written
//! binary keeps `forbid`. No hand-written code goes in this crate.
#![deny(unsafe_code)]

/// Code generated from `ui/app.slint`; the generator's style is outside this crate's lints.
#[allow(clippy::all)]
mod generated {
    slint::include_modules!();
}
pub use generated::SpikeWindow;
