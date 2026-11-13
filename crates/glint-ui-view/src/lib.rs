// SPDX-License-Identifier: MIT
//! The Slint-generated view of the Glint window (PLAN §12).
//!
//! Slint 1.18.1's generated code expands `ItemTreeVTable_static!`, which carries
//! `#[allow(unsafe_code)]`; that is a hard error under `#![forbid(unsafe_code)]` (E0453). So the
//! generated code lives here under `deny`, and every hand-written line of UI code stays in
//! `glint-ui` under `forbid` (ADR 0002). No hand-written logic goes in this crate.
#![deny(unsafe_code)]

/// Code generated from `ui/app.slint`; the generator's style is outside the workspace's lints,
/// including the `restriction` ones such as `unwrap_used`.
#[allow(clippy::all, clippy::restriction)]
mod generated {
    slint::include_modules!();
}
pub use generated::*;
