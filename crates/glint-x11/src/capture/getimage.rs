// SPDX-License-Identifier: MIT
//! Core `GetImage` capture (PLAN §5.2, Appendix A.3) over the raw connection A (ADR 0003): one
//! round trip per check, no allocation. Reading the root window is the `screen` backend; reading
//! the target window itself is `window-buffer` (Phase 5). With an MIT-SHM segment it can read
//! into shared memory instead: `screen-shm` (Phase 6).

use glint_core::pattern::Rect;

use super::shm::ShmSegment;
use super::{Capture, CaptureError, MAX_PIXELS};
use crate::format::PixelFormat;
use crate::wire::{self, RawConn};

pub struct GetImageCapture {
    conn: RawConn,
    drawable: u32,
    fmt: PixelFormat,
    /// The reply's image bytes, sized for the largest capture.
    data: Box<[u8]>,
    /// The MIT-SHM segment, when the server took one, and whether to read through it.
    shm: Option<ShmSegment>,
    use_shm: bool,
}

impl GetImageCapture {
    /// Reads from `drawable`, whose images have format `fmt`.
    pub fn new(conn: RawConn, drawable: u32, fmt: PixelFormat) -> Self {
        Self {
            conn,
            drawable,
            fmt,
            data: vec![0; MAX_PIXELS * 4].into_boxed_slice(),
            shm: None,
            use_shm: false,
        }
    }

    /// Adds an MIT-SHM segment to read through once [`Capture::set_shm`] says so.
    pub fn with_shm(mut self, segment: ShmSegment) -> Self {
        self.shm = Some(segment);
        self
    }
}

impl Capture for GetImageCapture {
    fn set_target(&mut self, drawable: u32, fmt: PixelFormat) {
        self.drawable = drawable;
        self.fmt = fmt;
    }

    fn set_shm(&mut self, on: bool) {
        self.use_shm = on && self.shm.is_some();
    }

    fn grab(&mut self, rect: Rect, out: &mut [u32]) -> Result<(), CaptureError> {
        let (Ok(x), Ok(y)) = (i16::try_from(rect.x), i16::try_from(rect.y)) else {
            return Err(CaptureError::Coordinates(rect));
        };
        if self.use_shm
            && let Some(segment) = self.shm.as_mut()
        {
            let image = segment.get(&mut self.conn, self.drawable, (x, y, rect.w, rect.h), &self.fmt)?;
            self.fmt.decode(image, rect.w, rect.h, out)?;
            return Ok(());
        }
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
