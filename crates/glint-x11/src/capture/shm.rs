// SPDX-License-Identifier: MIT
//! MIT-SHM capture, the `screen-shm` backend (PLAN §5.2): the server writes the image into a
//! shared `memfd` segment, so no pixel bytes cross the socket. Phase 0 found it no faster for a
//! 3×3 patch, so it's used for patterns larger than 32×32, or when a profile asks for it.
//!
//! The only module where `unsafe` is allowed (PLAN §13): mapping the segment, reading it, and
//! unmapping it. The segment is attached on an x11rb connection, which passes the file
//! descriptor; the hot connection A then asks for images into it over the raw protocol. A
//! segment is an X resource like a pixmap, which any client may name.
#![allow(unsafe_code)]

use std::ptr::NonNull;

use rustix::fs::MemfdFlags;
use rustix::mm::{MapFlags, ProtFlags};
use x11rb::connection::Connection;
use x11rb::protocol::shm::{self, ConnectionExt as _};

use super::CaptureError;
use crate::format::PixelFormat;
use crate::wire::{self, RawConn};

#[derive(Debug, thiserror::Error)]
pub enum ShmError {
    #[error("the X server has no MIT-SHM")]
    Missing,
    #[error("the X server's MIT-SHM {0}.{1} can't take a file descriptor (needs 1.2)")]
    NoFdPassing(u16, u16),
    #[error(transparent)]
    Io(#[from] rustix::io::Errno),
    #[error(transparent)]
    Connection(#[from] x11rb::errors::ConnectionError),
    #[error(transparent)]
    Reply(#[from] x11rb::errors::ReplyError),
    #[error(transparent)]
    Id(#[from] x11rb::errors::ReplyOrIdError),
}

/// A shared segment the X server writes images into, mapped read-only here.
pub struct ShmSegment {
    seg: u32,
    opcode: u8,
    ptr: NonNull<u8>,
    len: usize,
}

// SAFETY: the mapping belongs to this value alone; it is only read through `&mut self`, so moving
// the value to the hot thread can't create shared access.
unsafe impl Send for ShmSegment {}

impl ShmSegment {
    /// A segment of `len` bytes, attached to the server through `conn`.
    pub fn new(conn: &impl Connection, len: usize) -> Result<Self, ShmError> {
        let info = conn
            .extension_information(shm::X11_EXTENSION_NAME)?
            .ok_or(ShmError::Missing)?;
        let v = conn.shm_query_version()?.reply()?;
        if (v.major_version, v.minor_version) < (1, 2) {
            return Err(ShmError::NoFdPassing(v.major_version, v.minor_version));
        }
        let fd = rustix::fs::memfd_create("glint-shm", MemfdFlags::CLOEXEC)?;
        rustix::fs::ftruncate(&fd, len as u64)?;
        // SAFETY: a new shared mapping of a memfd this function just created and sized to `len`;
        // nothing else in this process maps it, and it is unmapped only in `drop`.
        let raw = unsafe { rustix::mm::mmap(std::ptr::null_mut(), len, ProtFlags::READ, MapFlags::SHARED, &fd, 0)? };
        let ptr = NonNull::new(raw.cast::<u8>()).ok_or(ShmError::Io(rustix::io::Errno::NOMEM))?;
        // From here on, dropping `segment` unmaps the memory, whatever fails next.
        let mut segment = Self {
            seg: 0,
            opcode: info.major_opcode,
            ptr,
            len,
        };
        segment.seg = conn.generate_id()?;
        // The server keeps its own reference to the file; ours closes when `fd` is sent.
        conn.shm_attach_fd(segment.seg, fd, false)?.check()?;
        Ok(segment)
    }

    /// Reads `w × h` pixels at `(x, y)` of `drawable` through connection `conn`, into the
    /// segment, and returns its image bytes in `fmt`'s layout.
    pub fn get(
        &mut self,
        conn: &mut RawConn,
        drawable: u32,
        (x, y, w, h): (i16, i16, u16, u16),
        fmt: &PixelFormat,
    ) -> Result<&[u8], CaptureError> {
        let len = fmt.image_len(w, h);
        if len > self.len {
            return Err(CaptureError::TooLarge(glint_core::pattern::Rect::new(
                i32::from(x),
                i32::from(y),
                w,
                h,
            )));
        }
        let seq = conn.send(&wire::shm_get_image(self.opcode, drawable, x, y, w, h, self.seg, 0))?;
        conn.wait_reply(seq, &mut [])?;
        // SAFETY: the mapping is `self.len` bytes and `len <= self.len`. The server writes the
        // segment only while a ShmGetImage on it is outstanding: it wrote this image before its
        // reply, and no request is outstanding while the returned slice (which borrows `self`
        // mutably) is alive, so the bytes can't change under it.
        Ok(unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), len) })
    }
}

impl Drop for ShmSegment {
    fn drop(&mut self) {
        // SAFETY: `ptr` and `len` are exactly the mapping made in `new`, and no slice into it can
        // outlive `self`. The server's side goes when its connection closes.
        let _ = unsafe { rustix::mm::munmap(self.ptr.as_ptr().cast(), self.len) };
    }
}
