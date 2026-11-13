// SPDX-License-Identifier: GPL-3.0-or-later
//! The Glint engine (`glintd`, PLAN §4): the hot loop, the controller, telemetry, and every
//! startup and stop path. `main.rs` only wires these together, so tests can drive each part.
#![forbid(unsafe_code)]

pub mod controller;
pub mod hot;
pub mod lifecycle;
pub mod lock;
pub mod rt;
pub mod store;
pub mod telemetry;
