// SPDX-License-Identifier: MIT
//! Glint's pure logic (PLAN §5, §11): color math and rules, the compiled lookup
//! table, sample patterns, the trigger state machine, and profile and settings
//! types. Nothing in this crate does I/O.
#![forbid(unsafe_code)]

pub mod color;
pub mod pattern;
pub mod profile;
pub mod schema;
pub mod settings;
pub mod trigger;
pub mod validate;

pub use color::{Class, Lut, Rgb};
pub use pattern::{SamplePattern, Verdict};
pub use profile::Profile;
pub use settings::Settings;
pub use trigger::{Hold, Step, Trigger, TriggerCfg};
