// SPDX-License-Identifier: GPL-3.0-or-later
//! Core `GetImage` capture (PLAN §5.2, Appendix A.3) over the raw connection A (ADR 0003): one
//! round trip per check, no allocation. Reading the root window is the `screen` backend; reading
//! the target window itself is `window-buffer` (Phase 5).

use glint_core::pattern::Rect;

use super::{Capture, CaptureError, MAX_PIXELS};
use crate::format::PixelFormat;
use crate::wire::{self, RawConn};

pub struct GetImageCapture {
    conn: RawConn,
    drawable: u32,
    fmt: PixelFormat,
    /// The reply's image bytes, sized for the largest capture.
    data: Box<[u8]>,
}

impl GetImageCapture {
    /// Reads from `drawable`, whose images have format `fmt`.
    pub fn new(conn: RawConn, drawable: u32, fmt: PixelFormat) -> Self {
        Self {
            conn,
            drawable,
            fmt,
            data: vec![0; MAX_PIXELS * 4].into_boxed_slice(),
        }
    }
}

impl Capture for GetImageCapture {
    fn set_target(&mut self, drawable: u32, fmt: PixelFormat) {
        self.drawable = drawable;
        self.fmt = fmt;
    }

    fn grab(&mut self, rect: Rect, out: &mut [u32]) -> Result<(), CaptureError> {
        let (Ok(x), Ok(y)) = (i16::try_from(rect.x), i16::try_from(rect.y)) else {
            return Err(CaptureError::Coordinates(rect));
        };
        let len = self.fmt.image_len(rect.w, rect.h);
        if len > self.data.len() {
            return Err(CaptureError::TooLarge(rect));
        }
        let seq = self.conn.send(&wire::get_image(self.drawable, x, y, rect.w, rect.h))?;
        let got = self.conn.wait_reply(seq, &mut self.data)?;
        self.fmt
            .decode(&self.data[..got.min(self.data.len())], rect.w, rect.h, out)?;
        Ok(())
    }
}
