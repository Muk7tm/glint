// SPDX-License-Identifier: GPL-3.0-or-later
//! Capture backends (PLAN §5.2). The hot loop owns one and calls [`Capture::grab`] once per
//! check; every backend reads into a caller-owned buffer and never allocates.

pub mod damage;
pub mod getimage;
pub mod shm;
pub mod window;

use glint_core::pattern::Rect;

use crate::format::FormatError;
use crate::wire::WireError;

/// The largest capture: a 63 × 63 pattern rectangle (ADR 0006).
pub const MAX_PIXELS: usize = 63 * 63;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error(transparent)]
    Wire(#[from] WireError),
    #[error(transparent)]
    Format(#[from] FormatError),
    #[error("{0:?} is larger than a capture can be")]
    TooLarge(Rect),
    #[error("{0:?} is outside X11's coordinate range")]
    Coordinates(Rect),
}

impl CaptureError {
    /// The window being read was hidden or closed (§5.2 "Errors"): stop with a reason, don't retry.
    pub fn is_target_gone(&self) -> bool {
        matches!(self, Self::Wire(WireError::X(e)) if e.is_target_gone())
    }
}

/// One way of reading the pixels under the sample pattern.
pub trait Capture: Send {
    /// Reads `rect`, in the drawable's coordinates, into `out` as `0xRRGGBB`, row by row.
    fn grab(&mut self, rect: Rect, out: &mut [u32]) -> Result<(), CaptureError>;
}
