// SPDX-License-Identifier: MIT
//! The allocation-free X protocol client for the hot connections, A (capture) and B (input)
//! (ADR 0003).
//!
//! Connection setup, Xauthority and display parsing come from x11rb-protocol's sans-I/O
//! helpers and allocate once, at startup. After that, every request is a fixed-size byte array
//! written with one `write`, and replies, errors and events are parsed in place from a fixed
//! read buffer: no allocation and no locks, so the hot loop may use it.
//!
//! Requests are encoded in this machine's byte order, which the setup request announced.

use std::io::{self, Read, Write};
use std::os::linux::net::SocketAddrExt as _;
use std::os::unix::net::{SocketAddr, UnixStream};
use std::time::Instant;

use rustix::net::RecvFlags;
use x11rb_protocol::connect::Connect;
use x11rb_protocol::protocol::xproto::Setup;
use x11rb_protocol::xauth::Family;

use crate::env::LocalDisplay;

/// The read buffer: room for the largest capture (63 × 63 pixels at 32 bits per pixel) and
/// whatever events arrive alongside it.
pub const READ_BUFFER: usize = 32 * 1024;

/// Core opcodes used on the hot connections.
const GET_IMAGE: u8 = 73;
const WARP_POINTER: u8 = 41;
const GET_INPUT_FOCUS: u8 = 43;
/// XTEST minor opcodes.
const XTEST_FAKE_INPUT: u8 = 2;
const XTEST_GRAB_CONTROL: u8 = 3;

/// An X error packet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct XError {
    pub code: u8,
    pub major: u8,
    pub minor: u16,
    pub sequence: u16,
}

impl XError {
    /// The window or drawable doesn't exist any more (BadWindow 3, BadDrawable 9) or isn't
    /// viewable (BadMatch 8): the target was closed or hidden (§5.2).
    pub fn is_target_gone(&self) -> bool {
        matches!(self.code, 3 | 8 | 9)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("X connection: {0}")]
    Io(#[from] io::Error),
    #[error("the X server closed the connection")]
    Closed,
    #[error("connecting to the X server: {0}")]
    Setup(String),
    #[error("X error {} (request {}.{}, sequence {})", .0.code, .0.major, .0.minor, .0.sequence)]
    X(XError),
    #[error("an X packet of {0} bytes doesn't fit the read buffer")]
    TooLarge(usize),
    #[error("got the reply to request {got} while waiting for {want}")]
    Sequence { got: u16, want: u16 },
}

/// One connection to the X server, with a fixed read buffer.
pub struct RawConn {
    stream: UnixStream,
    buf: Box<[u8]>,
    start: usize,
    end: usize,
    /// The sequence number of the last request sent (16 bits, like the server's).
    seq: u16,
    events: u64,
    async_errors: u64,
    last_async_error: Option<XError>,
    /// The first resource id this client may create, from the setup (0 for a test stream).
    first_id: u32,
    /// DAMAGE (§5.2): the event code of DamageNotify once followed, the watched rectangle
    /// `(x0, y0, x1, y1)`, and whether any damage, and any touching the rectangle, came in since
    /// they were last taken.
    damage_event: Option<u8>,
    damage_rect: Option<(i32, i32, i32, i32)>,
    damage_any: bool,
    damage_hit: bool,
}

impl RawConn {
    /// Connects to a local display: its Unix socket, then the abstract socket of the same name.
    pub fn connect(display: LocalDisplay) -> Result<(Self, Setup), WireError> {
        let path = display.socket_path();
        let stream = match UnixStream::connect(&path) {
            Ok(s) => s,
            Err(path_err) => SocketAddr::from_abstract_name(path.as_bytes())
                .and_then(|addr| UnixStream::connect_addr(&addr))
                .map_err(|_| path_err)?,
        };
        let host = rustix::system::uname().nodename().to_bytes().to_vec();
        // Like x11rb's own connection: a missing or unreadable Xauthority file means no
        // authorization data, not a failed connection (a fresh account or a CI runner has none,
        // and an X server started without -auth doesn't ask for any).
        let (name, data) = x11rb_protocol::xauth::get_auth(Family::LOCAL, &host, display.number)
            .unwrap_or(None)
            .unwrap_or_default();
        let (mut connect, setup_request) = Connect::with_authorization(name, data);
        (&stream).write_all(&setup_request)?;
        loop {
            let n = (&stream).read(connect.buffer())?;
            if n == 0 {
                return Err(WireError::Closed);
            }
            if connect.advance(n) {
                break;
            }
        }
        let setup = connect.into_setup().map_err(|e| WireError::Setup(e.to_string()))?;
        let mut conn = Self::from_stream(stream);
        // The lowest id in the client's range, stepped by the mask's lowest bit.
        let step = setup.resource_id_mask & setup.resource_id_mask.wrapping_neg();
        conn.first_id = setup.resource_id_base | step;
        Ok((conn, setup))
    }

    /// Wraps an already set-up stream (also used by the tests).
    pub fn from_stream(stream: UnixStream) -> Self {
        Self {
            stream,
            buf: vec![0; READ_BUFFER].into_boxed_slice(),
            start: 0,
            end: 0,
            seq: 0,
            events: 0,
            async_errors: 0,
            last_async_error: None,
            first_id: 0,
            damage_event: None,
            damage_rect: None,
            damage_any: false,
            damage_hit: false,
        }
    }

    /// Sends one complete request and returns its sequence number.
    pub fn send(&mut self, request: &[u8]) -> Result<u16, WireError> {
        self.send_batch(request, 1)
    }

    /// Sends `count` complete requests, back to back in `bytes`, with one write. Returns the
    /// sequence number of the last one.
    pub fn send_batch(&mut self, bytes: &[u8], count: u16) -> Result<u16, WireError> {
        (&self.stream).write_all(bytes)?;
        self.seq = self.seq.wrapping_add(count);
        Ok(self.seq)
    }

    /// Reads packets until the reply to request `want` arrives, copies as much of its data (the
    /// bytes after the 32-byte header) into `out` as fits, and returns the data's full length.
    /// Events are counted and skipped; errors for other requests are recorded.
    pub fn wait_reply(&mut self, want: u16, out: &mut [u8]) -> Result<usize, WireError> {
        loop {
            let len = self.next_packet()?;
            let p = &self.buf[self.start..self.start + len];
            match p[0] & 0x7f {
                0 => {
                    let e = parse_error(p);
                    self.start += len;
                    if e.sequence == want {
                        return Err(WireError::X(e));
                    }
                    self.record_async_error(e);
                }
                1 => {
                    let got = u16::from_ne_bytes([p[2], p[3]]);
                    let data = &p[32..];
                    let n = data.len().min(out.len());
                    out[..n].copy_from_slice(&data[..n]);
                    let full = data.len();
                    self.start += len;
                    if got != want {
                        return Err(WireError::Sequence { got, want });
                    }
                    return Ok(full);
                }
                _ => {
                    self.note_event(self.start);
                    self.start += len;
                }
            }
        }
    }

    /// Reads whatever the server has already sent, without waiting, and processes the complete
    /// packets: events are counted, errors recorded. Connection B sends no requests with replies,
    /// so it calls this regularly; otherwise events every client receives (MappingNotify) and
    /// errors would pile up unread.
    pub fn drain(&mut self) -> Result<(), WireError> {
        loop {
            self.compact();
            if self.end == self.buf.len() {
                break;
            }
            match rustix::net::recv(&self.stream, &mut self.buf[self.end..], RecvFlags::DONTWAIT) {
                Ok((0, _)) => return Err(WireError::Closed),
                Ok((n, _)) => self.end += n,
                Err(rustix::io::Errno::AGAIN) => break,
                Err(rustix::io::Errno::INTR) => {}
                Err(e) => return Err(io::Error::from(e).into()),
            }
        }
        while let Some(len) = self.complete_packet()? {
            let p = &self.buf[self.start..self.start + len];
            if p[0] & 0x7f == 0 {
                let e = parse_error(p);
                self.record_async_error(e);
            } else {
                self.note_event(self.start);
            }
            self.start += len;
        }
        Ok(())
    }

    /// A round trip (GetInputFocus): returns once the server has processed every earlier
    /// request, and surfaces their errors in [`take_async_error`](Self::take_async_error).
    pub fn sync(&mut self) -> Result<(), WireError> {
        let seq = self.send(&get_input_focus())?;
        self.wait_reply(seq, &mut [])?;
        Ok(())
    }

    /// Errors for requests without replies seen so far, and the latest one; clears the latter.
    pub fn take_async_error(&mut self) -> (u64, Option<XError>) {
        (self.async_errors, self.last_async_error.take())
    }

    /// Events received (and skipped) so far.
    pub fn events_seen(&self) -> u64 {
        self.events
    }

    fn record_async_error(&mut self, e: XError) {
        self.async_errors += 1;
        self.last_async_error = Some(e);
    }

    /// Moves unread bytes to the front of the buffer. Never reallocates.
    fn compact(&mut self) {
        if self.start > 0 {
            self.buf.copy_within(self.start..self.end, 0);
            self.end -= self.start;
            self.start = 0;
        }
    }

    /// The length of the packet at `start` if it's completely buffered.
    /// The first resource id this client may create (DAMAGE's object on connection A), or
    /// `None` for a connection without a setup.
    pub fn first_id(&self) -> Option<u32> {
        (self.first_id != 0).then_some(self.first_id)
    }

    /// Notes DamageNotify events (code `event`) from now on (§5.2).
    pub fn follow_damage(&mut self, event: u8) {
        self.damage_event = Some(event);
    }

    /// The rectangle whose damage counts as a hit, or none.
    pub fn set_damage_rect(&mut self, rect: Option<(i16, i16, u16, u16)>) {
        self.damage_rect = rect.map(|(x, y, w, h)| {
            let (x, y) = (i32::from(x), i32::from(y));
            (x, y, x + i32::from(w), y + i32::from(h))
        });
        self.damage_hit = false;
    }

    /// Whether any damage, and any touching the rectangle, came in since the last call.
    pub fn take_damage(&mut self) -> (bool, bool) {
        let seen = (self.damage_any, self.damage_hit);
        (self.damage_any, self.damage_hit) = (false, false);
        seen
    }

    /// Whether damage touching the rectangle came in and hasn't been taken yet.
    pub fn damage_pending(&self) -> bool {
        self.damage_hit
    }

    /// Waits until the server has sent something, or `deadline` passes (`false`).
    pub fn wait_readable(&mut self, deadline: Instant) -> Result<bool, WireError> {
        use rustix::event::{PollFd, PollFlags, Timespec};
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let timeout = Timespec {
                tv_sec: i64::try_from(left.as_secs()).unwrap_or(i64::MAX),
                tv_nsec: i64::from(left.subsec_nanos()),
            };
            let mut fds = [PollFd::new(&self.stream, PollFlags::IN)];
            match rustix::event::poll(&mut fds, Some(&timeout)) {
                Ok(0) => return Ok(false),
                Ok(_) => return Ok(true),
                Err(rustix::io::Errno::INTR) => {}
                Err(e) => return Err(io::Error::from(e).into()),
            }
        }
    }

    /// Counts the event at `at` in the buffer, and notes DamageNotify: its area is the bounding
    /// box of the damage so far, in the damaged drawable's coordinates.
    fn note_event(&mut self, at: usize) {
        self.events += 1;
        let p = &self.buf[at..at + 32];
        if Some(p[0] & 0x7f) != self.damage_event {
            return;
        }
        let x = i32::from(i16::from_ne_bytes([p[16], p[17]]));
        let y = i32::from(i16::from_ne_bytes([p[18], p[19]]));
        let w = i32::from(u16::from_ne_bytes([p[20], p[21]]));
        let h = i32::from(u16::from_ne_bytes([p[22], p[23]]));
        self.damage_any = true;
        if let Some((x0, y0, x1, y1)) = self.damage_rect
            && x < x1
            && x + w > x0
            && y < y1
            && y + h > y0
        {
            self.damage_hit = true;
        }
    }

    fn complete_packet(&self) -> Result<Option<usize>, WireError> {
        let have = self.end - self.start;
        if have < 32 {
            return Ok(None);
        }
        let len = packet_len(&self.buf[self.start..self.start + 32]);
        if len > self.buf.len() {
            return Err(WireError::TooLarge(len));
        }
        Ok((have >= len).then_some(len))
    }

    /// Blocks until the next packet is completely buffered and returns its length.
    fn next_packet(&mut self) -> Result<usize, WireError> {
        self.fill(32)?;
        let len = packet_len(&self.buf[self.start..self.start + 32]);
        if len > self.buf.len() {
            return Err(WireError::TooLarge(len));
        }
        self.fill(len)?;
        Ok(len)
    }

    /// Makes sure at least `n` unread bytes are buffered.
    fn fill(&mut self, n: usize) -> Result<(), WireError> {
        if self.end - self.start >= n {
            return Ok(());
        }
        self.compact();
        while self.end < n {
            match (&self.stream).read(&mut self.buf[self.end..]) {
                Ok(0) => return Err(WireError::Closed),
                Ok(r) => self.end += r,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}

/// The total length of the packet whose first 32 bytes are `header`: replies (1) and generic
/// events (35) carry a length in 4-byte units; errors and every other event are 32 bytes.
fn packet_len(header: &[u8]) -> usize {
    match header[0] & 0x7f {
        1 | 35 => 32 + 4 * u32::from_ne_bytes([header[4], header[5], header[6], header[7]]) as usize,
        _ => 32,
    }
}

fn parse_error(p: &[u8]) -> XError {
    XError {
        code: p[1],
        sequence: u16::from_ne_bytes([p[2], p[3]]),
        minor: u16::from_ne_bytes([p[8], p[9]]),
        major: p[10],
    }
}

/// `GetImage` in Z-pixmap format with all planes: the capture request.
pub fn get_image(drawable: u32, x: i16, y: i16, width: u16, height: u16) -> [u8; 20] {
    let mut r = [0u8; 20];
    r[0] = GET_IMAGE;
    r[1] = 2; // ZPixmap
    r[2..4].copy_from_slice(&5u16.to_ne_bytes());
    r[4..8].copy_from_slice(&drawable.to_ne_bytes());
    r[8..10].copy_from_slice(&x.to_ne_bytes());
    r[10..12].copy_from_slice(&y.to_ne_bytes());
    r[12..14].copy_from_slice(&width.to_ne_bytes());
    r[14..16].copy_from_slice(&height.to_ne_bytes());
    r[16..20].copy_from_slice(&u32::MAX.to_ne_bytes());
    r
}

/// MIT-SHM `ShmGetImage` (minor 4) of a Z-pixmap with every plane, into segment `seg` at
/// `offset`; `opcode` is the extension's major opcode. The reply carries no pixels.
#[allow(clippy::too_many_arguments)]
pub fn shm_get_image(
    opcode: u8,
    drawable: u32,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    seg: u32,
    offset: u32,
) -> [u8; 32] {
    let mut r = [0u8; 32];
    r[0] = opcode;
    r[1] = 4; // ShmGetImage
    r[2..4].copy_from_slice(&8u16.to_ne_bytes());
    r[4..8].copy_from_slice(&drawable.to_ne_bytes());
    r[8..10].copy_from_slice(&x.to_ne_bytes());
    r[10..12].copy_from_slice(&y.to_ne_bytes());
    r[12..14].copy_from_slice(&width.to_ne_bytes());
    r[14..16].copy_from_slice(&height.to_ne_bytes());
    r[16..20].copy_from_slice(&u32::MAX.to_ne_bytes());
    r[20] = 2; // ZPixmap
    r[24..28].copy_from_slice(&seg.to_ne_bytes());
    r[28..32].copy_from_slice(&offset.to_ne_bytes());
    r
}

/// DAMAGE `QueryVersion` (minor 0) for 1.1, which a client must send before other DAMAGE
/// requests.
pub fn damage_query_version(opcode: u8) -> [u8; 12] {
    let mut r = [0u8; 12];
    r[0] = opcode;
    r[2..4].copy_from_slice(&3u16.to_ne_bytes());
    r[4..8].copy_from_slice(&1u32.to_ne_bytes());
    r[8..12].copy_from_slice(&1u32.to_ne_bytes());
    r
}

/// DAMAGE `Create` (minor 1): damage object `damage` on `drawable` at report `level` (2 is
/// BoundingBox).
pub fn damage_create(opcode: u8, damage: u32, drawable: u32, level: u8) -> [u8; 16] {
    let mut r = [0u8; 16];
    r[0] = opcode;
    r[1] = 1;
    r[2..4].copy_from_slice(&4u16.to_ne_bytes());
    r[4..8].copy_from_slice(&damage.to_ne_bytes());
    r[8..12].copy_from_slice(&drawable.to_ne_bytes());
    r[12] = level;
    r
}

/// DAMAGE `Destroy` (minor 2).
pub fn damage_destroy(opcode: u8, damage: u32) -> [u8; 8] {
    let mut r = [0u8; 8];
    r[0] = opcode;
    r[1] = 2;
    r[2..4].copy_from_slice(&2u16.to_ne_bytes());
    r[4..8].copy_from_slice(&damage.to_ne_bytes());
    r
}

/// DAMAGE `Subtract` (minor 3) with no repair or parts region: clears all of `damage`, so the
/// next change sends a new DamageNotify.
pub fn damage_subtract(opcode: u8, damage: u32) -> [u8; 16] {
    let mut r = [0u8; 16];
    r[0] = opcode;
    r[1] = 3;
    r[2..4].copy_from_slice(&4u16.to_ne_bytes());
    r[4..8].copy_from_slice(&damage.to_ne_bytes());
    r
}

/// The size of one `XTestFakeInput` request.
pub const FAKE_INPUT_LEN: usize = 36;

/// `XTestFakeInput` with the current time and no motion: a button or key press or release.
pub fn fake_input(xtest_opcode: u8, event_type: u8, detail: u8, root: u32) -> [u8; FAKE_INPUT_LEN] {
    let mut r = [0u8; FAKE_INPUT_LEN];
    r[0] = xtest_opcode;
    r[1] = XTEST_FAKE_INPUT;
    r[2..4].copy_from_slice(&9u16.to_ne_bytes());
    r[4] = event_type;
    r[5] = detail;
    // time (8..12) = CurrentTime (0)
    r[12..16].copy_from_slice(&root.to_ne_bytes());
    // root x/y and device id stay 0
    r
}

/// `XTestGrabControl`: with `impervious`, another client's server grab can't stall this
/// connection's input (§6).
pub fn grab_control(xtest_opcode: u8, impervious: bool) -> [u8; 8] {
    let mut r = [0u8; 8];
    r[0] = xtest_opcode;
    r[1] = XTEST_GRAB_CONTROL;
    r[2..4].copy_from_slice(&2u16.to_ne_bytes());
    r[4] = u8::from(impervious);
    r
}

/// `WarpPointer` to `(x, y)` relative to `dst` (the root for the watched spot).
pub fn warp_pointer(dst: u32, x: i16, y: i16) -> [u8; 24] {
    let mut r = [0u8; 24];
    r[0] = WARP_POINTER;
    r[2..4].copy_from_slice(&6u16.to_ne_bytes());
    // src_window (4..8) = None; src x/y/width/height (12..20) = 0
    r[8..12].copy_from_slice(&dst.to_ne_bytes());
    r[20..22].copy_from_slice(&x.to_ne_bytes());
    r[22..24].copy_from_slice(&y.to_ne_bytes());
    r
}

/// `GetInputFocus`: the cheapest request with a reply, used for round trips.
pub fn get_input_focus() -> [u8; 4] {
    let mut r = [GET_INPUT_FOCUS, 0, 0, 0];
    r[2..4].copy_from_slice(&1u16.to_ne_bytes());
    r
}

#[cfg(test)]
mod tests {
    use x11rb_protocol::protocol::xproto::{GetImageRequest, GetInputFocusRequest, ImageFormat, WarpPointerRequest};
    use x11rb_protocol::protocol::xtest::{FakeInputRequest, GrabControlRequest};

    use super::*;

    fn joined(pieces: &[std::borrow::Cow<'_, [u8]>]) -> Vec<u8> {
        pieces.iter().flat_map(|p| p.iter().copied()).collect()
    }

    #[test]
    fn requests_match_x11rb_byte_for_byte() {
        let (bytes, _) = GetImageRequest {
            format: ImageFormat::Z_PIXMAP,
            drawable: 0x1234_5678,
            x: -3,
            y: 1079,
            width: 63,
            height: 5,
            plane_mask: u32::MAX,
        }
        .serialize();
        assert_eq!(get_image(0x1234_5678, -3, 1079, 63, 5).to_vec(), joined(&bytes));

        for (event_type, detail) in [(4u8, 1u8), (5, 9), (2, 26), (3, 255)] {
            let (bytes, _) = FakeInputRequest {
                type_: event_type,
                detail,
                time: 0,
                root: 0x0000_0757,
                root_x: 0,
                root_y: 0,
                deviceid: 0,
            }
            .serialize(132);
            assert_eq!(fake_input(132, event_type, detail, 0x757).to_vec(), joined(&bytes));
        }

        for impervious in [true, false] {
            let (bytes, _) = GrabControlRequest { impervious }.serialize(132);
            assert_eq!(grab_control(132, impervious).to_vec(), joined(&bytes));
        }

        let (bytes, _) = WarpPointerRequest {
            src_window: 0,
            dst_window: 0x757,
            src_x: 0,
            src_y: 0,
            src_width: 0,
            src_height: 0,
            dst_x: 960,
            dst_y: -540,
        }
        .serialize();
        assert_eq!(warp_pointer(0x757, 960, -540).to_vec(), joined(&bytes));

        let (bytes, _) = GetInputFocusRequest.serialize();
        assert_eq!(get_input_focus().to_vec(), joined(&bytes));
    }

    /// Packet builders for the parsing tests.
    fn reply(sequence: u16, data: &[u8]) -> Vec<u8> {
        assert_eq!(data.len() % 4, 0);
        let mut p = vec![0u8; 32];
        p[0] = 1;
        p[2..4].copy_from_slice(&sequence.to_ne_bytes());
        p[4..8].copy_from_slice(&((data.len() / 4) as u32).to_ne_bytes());
        p.extend_from_slice(data);
        p
    }

    fn error(code: u8, sequence: u16, major: u8) -> Vec<u8> {
        let mut p = vec![0u8; 32];
        p[1] = code;
        p[2..4].copy_from_slice(&sequence.to_ne_bytes());
        p[10] = major;
        p
    }

    fn event(kind: u8) -> Vec<u8> {
        let mut p = vec![0u8; 32];
        p[0] = kind;
        p
    }

    fn generic_event(extra_words: u32) -> Vec<u8> {
        let mut p = vec![0u8; 32 + 4 * extra_words as usize];
        p[0] = 35;
        p[4..8].copy_from_slice(&extra_words.to_ne_bytes());
        p
    }

    fn pair() -> (RawConn, UnixStream) {
        let (ours, server) = UnixStream::pair().unwrap();
        (RawConn::from_stream(ours), server)
    }

    #[test]
    fn a_reply_after_events_and_an_unrelated_error() {
        let (mut conn, mut server) = pair();
        let mut stream = Vec::new();
        stream.extend(event(34)); // MappingNotify, which every client gets
        stream.extend(generic_event(3));
        stream.extend(error(8, 4, 132)); // an earlier request without a reply failed
        stream.extend(reply(5, &[1, 2, 3, 4, 5, 6, 7, 8]));
        server.write_all(&stream).unwrap();
        let mut out = [0u8; 4];
        assert_eq!(conn.wait_reply(5, &mut out).unwrap(), 8); // the full length, though only 4 fit
        assert_eq!(out, [1, 2, 3, 4]);
        assert_eq!(conn.events_seen(), 2);
        assert_eq!(
            conn.take_async_error(),
            (
                1,
                Some(XError {
                    code: 8,
                    major: 132,
                    minor: 0,
                    sequence: 4
                })
            )
        );
        assert_eq!(conn.take_async_error(), (1, None));
    }

    #[test]
    fn an_error_for_the_awaited_request() {
        let (mut conn, mut server) = pair();
        server.write_all(&error(9, 7, 73)).unwrap();
        match conn.wait_reply(7, &mut []) {
            Err(WireError::X(e)) => {
                assert_eq!(
                    e,
                    XError {
                        code: 9,
                        major: 73,
                        minor: 0,
                        sequence: 7
                    }
                );
                assert!(e.is_target_gone());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_reply_for_another_request_is_an_error() {
        let (mut conn, mut server) = pair();
        server.write_all(&reply(3, &[])).unwrap();
        assert!(matches!(
            conn.wait_reply(4, &mut []),
            Err(WireError::Sequence { got: 3, want: 4 })
        ));
    }

    #[test]
    fn packets_split_across_reads_and_a_closed_connection() {
        let (mut conn, mut server) = pair();
        let r = reply(1, &[9; 12]);
        server.write_all(&r[..10]).unwrap();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            server.write_all(&r[10..]).unwrap();
            server
        });
        let mut out = [0u8; 12];
        assert_eq!(conn.wait_reply(1, &mut out).unwrap(), 12);
        assert_eq!(out, [9; 12]);
        drop(writer.join().unwrap());
        assert!(matches!(conn.wait_reply(2, &mut []), Err(WireError::Closed)));
    }

    #[test]
    fn oversized_packets_are_refused() {
        let (mut conn, mut server) = pair();
        let mut header = reply(1, &[]);
        header[4..8].copy_from_slice(&((READ_BUFFER / 4) as u32).to_ne_bytes());
        server.write_all(&header).unwrap();
        assert!(matches!(conn.wait_reply(1, &mut []), Err(WireError::TooLarge(_))));
    }

    #[test]
    fn drain_reads_without_waiting() {
        let (mut conn, mut server) = pair();
        conn.drain().unwrap(); // nothing sent: returns at once
        assert_eq!(conn.events_seen(), 0);
        let mut stream = event(34);
        stream.extend(error(2, 1, 132));
        stream.extend(&event(12)[..16]); // half an event
        server.write_all(&stream).unwrap();
        conn.drain().unwrap();
        assert_eq!(conn.events_seen(), 1);
        assert_eq!(conn.take_async_error().0, 1);
        server.write_all(&event(12)[16..]).unwrap();
        conn.drain().unwrap();
        assert_eq!(conn.events_seen(), 2);
        drop(server);
        // Another test may be forking a child (Xvfb) right now, which holds a copy of the
        // socket until it execs: the close then shows up a moment later.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut closed = false;
        while !closed && std::time::Instant::now() < deadline {
            closed = matches!(conn.drain(), Err(WireError::Closed));
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(closed);
    }

    #[test]
    fn sequence_numbers_count_every_request_and_wrap() {
        let (mut conn, server) = pair();
        assert_eq!(conn.send(&get_input_focus()).unwrap(), 1);
        let two = [fake_input(132, 4, 1, 1), fake_input(132, 5, 1, 1)].concat();
        assert_eq!(conn.send_batch(&two, 2).unwrap(), 3);
        conn.seq = u16::MAX;
        assert_eq!(conn.send(&get_input_focus()).unwrap(), 0);
        drop(server);
    }
}
