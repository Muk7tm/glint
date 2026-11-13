// SPDX-License-Identifier: GPL-3.0-or-later
//! The controller (PLAN §4.2): IPC, settings and profiles, lookup-table builds, X connection C
//! (monitors, windows, focus, hotkeys, picking). It is the only thread that changes what the hot
//! thread does, through [`crate::hot::HotHandle`].

use std::sync::Arc;
use std::time::Duration;

use glint_core::Lut;
use glint_core::pattern::PatternError;
use glint_core::profile::{ActionKind, ClickAt, Modifier, Profile};
use glint_x11::inject::{Action, Input, MAX_MODIFIERS};
use glint_x11::keymap::{Keymap, keysym};

use crate::hot::CompiledProfile;

#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    #[error(transparent)]
    Pattern(#[from] PatternError),
    #[error("the key action has no key")]
    NoKey,
    #[error("the keyboard has no {0:?} key")]
    NoModifier(Modifier),
}

/// The keysym a modifier name stands for.
fn modifier_keysym(m: Modifier) -> u32 {
    match m {
        Modifier::Shift => keysym::SHIFT_L,
        Modifier::Ctrl => keysym::CONTROL_L,
        Modifier::Alt => keysym::ALT_L,
        Modifier::Super => keysym::SUPER_L,
    }
}

/// Turns a validated profile into what the hot loop runs. The lookup table is built separately
/// (it takes a few hundred milliseconds); `watched_spot` is where *Click at: Watched spot* moves
/// the pointer, in root coordinates.
pub fn compile(
    profile: &Profile,
    keymap: &Keymap,
    lut: Arc<Lut>,
    watched_spot: (i16, i16),
) -> Result<CompiledProfile, CompileError> {
    let a = &profile.action;
    let input = match a.kind {
        ActionKind::Mouse => Input::Button(a.button.x_button()),
        ActionKind::Key => Input::Key(a.key.as_ref().ok_or(CompileError::NoKey)?.keycode),
    };
    let mut modifiers = [0u8; MAX_MODIFIERS];
    let n = a.modifiers.len().min(MAX_MODIFIERS);
    for (slot, &m) in modifiers.iter_mut().zip(&a.modifiers) {
        *slot = keymap.keycode(modifier_keysym(m)).ok_or(CompileError::NoModifier(m))?;
    }
    let warp = (a.click_at == ClickAt::WatchedSpot).then_some(watched_spot);
    let action = Action::new(input, Duration::from_millis(a.hold_ms.into()))
        .with_modifiers(&modifiers[..n])
        .with_warp(warp);
    Ok(CompiledProfile {
        revision: 0,
        lut,
        pattern: profile.pattern()?,
        trigger: profile.trigger_cfg(),
        action,
        period: profile.sampling.speed.period(),
    })
}

/// Every keycode a profile's action could hold, for the emergency and defensive releases (C10).
pub fn action_keycodes(profile: &Profile, keymap: &Keymap) -> Vec<u8> {
    let mut out = keymap.modifier_keycodes();
    if let Some(k) = &profile.action.key {
        out.push(k.keycode);
    }
    out.sort_unstable();
    out.dedup();
    out
}
