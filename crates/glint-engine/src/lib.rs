// SPDX-License-Identifier: GPL-3.0-or-later
//! The Glint engine (`glintd`, PLAN §4): the hot loop, the controller, telemetry, and every
//! startup and stop path. `main.rs` only wires these together, so tests can drive each part.
#![forbid(unsafe_code)]

/// Debug builds check that the hot loop never allocates (§5.6, ADR 0003): any allocation inside
/// `assert_no_alloc` aborts with a message.
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

pub mod controller;
pub mod hot;
pub mod lifecycle;
pub mod lock;
pub mod pick;
pub mod rt;
pub mod store;
pub mod target;
pub mod telemetry;
