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
    /// DAMAGE, once negotiated on this connection.
    damage: Option<Damage>,
}

/// The DAMAGE extension on connection A: its opcode, the one damage object's id, and the
/// drawable that object exists on, if any.
struct Damage {
    opcode: u8,
    id: u32,
    on: Option<u32>,
}

/// DAMAGE's BoundingBox report level: a DamageNotify whenever the damaged area's bounding box
/// grows (§5.2).
const BOUNDING_BOX: u8 = 2;

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
            damage: None,
        }
    }

    /// Negotiates DAMAGE (major opcode `opcode`, events from `first_event`) on this connection,
    /// so the `damage` backend can follow changes (§5.2). Done once, before the hot thread runs;
    /// on an error the capture works on without it.
    pub fn negotiate_damage(&mut self, opcode: u8, first_event: u8) -> Result<(), CaptureError> {
        let Some(id) = self.conn.first_id() else {
            return Ok(());
        };
        let seq = self.conn.send(&wire::damage_query_version(opcode))?;
        self.conn.wait_reply(seq, &mut [])?;
        self.conn.follow_damage(first_event);
        self.damage = Some(Damage { opcode, id, on: None });
        Ok(())
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

    fn watch_damage(&mut self, rect: Option<Rect>) -> Result<bool, CaptureError> {
        let Some(d) = self.damage.as_mut() else {
            return Ok(false);
        };
        let Some(rect) = rect else {
            if d.on.take().is_some() {
                self.conn.send(&wire::damage_destroy(d.opcode, d.id))?;
            }
            self.conn.set_damage_rect(None);
            return Ok(false);
        };
        let (Ok(x), Ok(y)) = (i16::try_from(rect.x), i16::try_from(rect.y)) else {
            return Err(CaptureError::Coordinates(rect));
        };
        if d.on != Some(self.drawable) {
            if d.on.is_some() {
                self.conn.send(&wire::damage_destroy(d.opcode, d.id))?;
            }
            self.conn
                .send(&wire::damage_create(d.opcode, d.id, self.drawable, BOUNDING_BOX))?;
            d.on = Some(self.drawable);
        }
        self.conn.set_damage_rect(Some((x, y, rect.w, rect.h)));
        Ok(true)
    }

    fn wait_damage(&mut self, deadline: std::time::Instant) -> Result<bool, CaptureError> {
        let Some(d) = self.damage.as_ref() else {
            return Ok(false);
        };
        let (opcode, id) = (d.opcode, d.id);
        loop {
            // Events already read (behind the last reply) first, then whatever has arrived.
            self.conn.drain()?;
            let (any, hit) = self.conn.take_damage();
            // A bounding box only reports growth: clear it after every report, so the next
            // change reports again. The check that follows reads after the clearing, so nothing
            // cleared goes unseen.
            if any {
                self.conn.send(&wire::damage_subtract(opcode, id))?;
            }
            if hit {
                return Ok(true);
            }
            if !self.conn.wait_readable(deadline)? {
                return Ok(false);
            }
        }
    }

    fn damage_pending(&self) -> bool {
        self.conn.damage_pending()
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
