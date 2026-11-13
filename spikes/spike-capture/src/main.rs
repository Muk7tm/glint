// SPDX-License-Identifier: GPL-3.0-or-later
//! Phase 0 spike (PLAN §19): what does one check cost on this machine?
//!
//! ```text
//! spike-capture info
//!     Server, pixel formats, extension versions, monitors and compositor.
//! spike-capture bench [--target root|window] [--method x11rb|raw|shm] [--count N] [--label TEXT]
//!     Times N GetImage round trips for a 3×3 patch at the centre of a small
//!     test window, read through the root window (screen backend) or the test
//!     window itself (window-buffer backend), and counts heap allocations per check.
//! spike-capture tax [--rate 0|500|2000|max] [--secs S] [--method raw|x11rb] [--label TEXT]
//!     Times GetInputFocus round trips from a second client while a load thread
//!     captures at the given rate: the X server tax (PLAN §5.6).
//! ```
//!
//! Read-only: it never injects input. The test window is a 64×64 override-redirect
//! square in the top-right corner, so it takes no focus.
#![deny(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::composite::ConnectionExt as _;
use x11rb::protocol::damage::ConnectionExt as _;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::screensaver::ConnectionExt as _;
use x11rb::protocol::shm::ConnectionExt as _;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xinput::ConnectionExt as _;
use x11rb::protocol::xproto::{ConnectionExt as _, CreateWindowAux, ImageFormat, Window, WindowClass};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb_protocol::xauth::Family;

type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

// ---------------------------------------------------------------- allocation counter

struct CountingAlloc;
static ALLOCS: AtomicU64 = AtomicU64::new(0);

// SAFETY: every method forwards unchanged to the system allocator, with the
// caller's arguments and contract; the only addition is a relaxed counter.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: same contract as this function.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: same contract as this function.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: same contract as this function.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: same contract as this function.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

// ---------------------------------------------------------------- test window

const PATCH: u16 = 3;
const WIN_SIZE: u16 = 64;
/// The plan's example green (Appendix B), as a 24-bit TrueColor pixel value.
const COLOR: u32 = 0x3B_D1_6F;

struct TestWindow {
    // Kept open for the whole run: the window lives as long as this connection.
    _conn: RustConnection,
    win: Window,
    root: Window,
    x: i16,
    y: i16,
}

impl TestWindow {
    fn open() -> Res<Self> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let x = i16::try_from(screen.width_in_pixels - WIN_SIZE - 32)?;
        let y = 32;
        let win = conn.generate_id()?;
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            win,
            root,
            x,
            y,
            WIN_SIZE,
            WIN_SIZE,
            0,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new().background_pixel(COLOR).override_redirect(1),
        )?;
        conn.map_window(win)?;
        conn.sync()?;
        // Give a compositor a few frames to paint it.
        std::thread::sleep(Duration::from_millis(300));
        Ok(Self {
            _conn: conn,
            win,
            root,
            x,
            y,
        })
    }

    /// Drawable and patch origin for the chosen target.
    fn spot(&self, target: Target) -> (Window, i16, i16) {
        let c = i16::try_from(WIN_SIZE / 2 - PATCH / 2).unwrap_or(0);
        match target {
            Target::Root => (self.root, self.x + c, self.y + c),
            Target::Window => (self.win, c, c),
        }
    }
}

/// Centre pixel of a 3×3 Z-pixmap at 32 bits per pixel, LSB first (checked by `info`).
fn center_pixel(data: &[u8]) -> u32 {
    let i = usize::from((PATCH / 2) * PATCH + PATCH / 2) * 4;
    u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) & 0x00FF_FFFF
}

// ---------------------------------------------------------------- capture methods

trait Capture {
    /// One GetImage round trip; returns the centre pixel as 0xRRGGBB.
    fn grab(&mut self) -> Res<u32>;
}

/// x11rb's high-level API, as Appendix A.3 sketches it.
struct X11rbCapture {
    conn: RustConnection,
    drawable: Window,
    x: i16,
    y: i16,
}

impl Capture for X11rbCapture {
    fn grab(&mut self) -> Res<u32> {
        let reply = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, self.drawable, self.x, self.y, PATCH, PATCH, !0)?
            .reply()?;
        Ok(center_pixel(&reply.data))
    }
}

/// A minimal X client for the hot path: fixed buffers on a blocking Unix socket.
/// Connection setup and Xauthority come from x11rb-protocol's sans-I/O helpers;
/// after that no request or reply allocates.
struct RawConn {
    stream: UnixStream,
    buf: Vec<u8>,
    start: usize,
    end: usize,
    seq: u16,
}

impl RawConn {
    fn connect() -> Res<Self> {
        let parsed = x11rb_protocol::parse_display::parse_display(None)?;
        let stream = UnixStream::connect(format!("/tmp/.X11-unix/X{}", parsed.display))?;
        let uname = rustix::system::uname();
        let host = uname.nodename().to_bytes().to_vec();
        let (mut connect, setup_request) = x11rb_protocol::connect::Connect::new(Family::LOCAL, &host, parsed.display)?;
        (&stream).write_all(&setup_request)?;
        loop {
            let n = (&stream).read(connect.buffer())?;
            if n == 0 {
                return Err("the X server closed the connection during setup".into());
            }
            if connect.advance(n) {
                break;
            }
        }
        let _setup = connect.into_setup()?;
        Ok(Self {
            stream,
            buf: vec![0; 8192],
            start: 0,
            end: 0,
            seq: 0,
        })
    }

    fn send(&mut self, request: &[u8]) -> Res<u16> {
        (&self.stream).write_all(request)?;
        self.seq = self.seq.wrapping_add(1);
        Ok(self.seq)
    }

    /// Make sure at least `n` bytes are buffered. Never reallocates.
    fn fill(&mut self, n: usize) -> Res<()> {
        if self.end - self.start >= n {
            return Ok(());
        }
        if self.start > 0 {
            self.buf.copy_within(self.start..self.end, 0);
            self.end -= self.start;
            self.start = 0;
        }
        if n > self.buf.len() {
            return Err("packet larger than the read buffer".into());
        }
        while self.end < n {
            let r = (&self.stream).read(&mut self.buf[self.end..])?;
            if r == 0 {
                return Err("the X server closed the connection".into());
            }
            self.end += r;
        }
        Ok(())
    }

    /// Read packets until the reply to `seq` arrives and copy its data into `out`.
    /// Events (which arrive even unselected, e.g. MappingNotify) are skipped.
    fn wait_reply(&mut self, seq: u16, out: &mut [u8]) -> Res<usize> {
        loop {
            self.fill(32)?;
            let p = &self.buf[self.start..self.end];
            let kind = p[0];
            if kind == 0 {
                let (code, major) = (p[1], p[10]);
                let s = u16::from_ne_bytes([p[2], p[3]]);
                self.start += 32;
                return Err(format!("X error {code} (major opcode {major}, sequence {s})").into());
            }
            let extra = if kind == 1 || kind == 35 {
                u32::from_ne_bytes([p[4], p[5], p[6], p[7]]) as usize * 4
            } else {
                0
            };
            self.fill(32 + extra)?;
            let p = &self.buf[self.start..self.start + 32 + extra];
            if kind == 1 {
                let s = u16::from_ne_bytes([p[2], p[3]]);
                let n = extra.min(out.len());
                out[..n].copy_from_slice(&p[32..32 + n]);
                self.start += 32 + extra;
                if s != seq {
                    return Err(format!("reply for sequence {s}, expected {seq}").into());
                }
                return Ok(n);
            }
            self.start += 32 + extra;
        }
    }
}

struct RawCapture {
    conn: RawConn,
    request: [u8; 20],
    data: [u8; 256],
}

impl RawCapture {
    fn new(conn: RawConn, drawable: Window, x: i16, y: i16) -> Self {
        // GetImage (core opcode 73), ZPixmap, request length 5 units, native byte order.
        let mut request = [0u8; 20];
        request[0] = 73;
        request[1] = 2;
        request[2..4].copy_from_slice(&5u16.to_ne_bytes());
        request[4..8].copy_from_slice(&drawable.to_ne_bytes());
        request[8..10].copy_from_slice(&x.to_ne_bytes());
        request[10..12].copy_from_slice(&y.to_ne_bytes());
        request[12..14].copy_from_slice(&PATCH.to_ne_bytes());
        request[14..16].copy_from_slice(&PATCH.to_ne_bytes());
        request[16..20].copy_from_slice(&u32::MAX.to_ne_bytes());
        Self {
            conn,
            request,
            data: [0; 256],
        }
    }
}

impl Capture for RawCapture {
    fn grab(&mut self) -> Res<u32> {
        let seq = self.conn.send(&self.request)?;
        let n = self.conn.wait_reply(seq, &mut self.data)?;
        if n < usize::from(PATCH * PATCH) * 4 {
            return Err("short GetImage reply".into());
        }
        Ok(center_pixel(&self.data))
    }
}

/// Control load: a trivial GetInputFocus (core opcode 43) over the raw path, so the
/// server tax of "a request" can be told apart from the tax of "a GetImage".
struct RawFocus {
    conn: RawConn,
    data: [u8; 64],
}

impl Capture for RawFocus {
    fn grab(&mut self) -> Res<u32> {
        let seq = self.conn.send(&[43, 0, 1, 0])?;
        self.conn.wait_reply(seq, &mut self.data)?;
        Ok(0)
    }
}

/// MIT-SHM `ShmGetImage` into a memfd segment passed to the server.
struct ShmCapture {
    conn: RustConnection,
    map: memmap2::Mmap,
    seg: u32,
    drawable: Window,
    x: i16,
    y: i16,
}

impl ShmCapture {
    fn new(conn: RustConnection, drawable: Window, x: i16, y: i16) -> Res<Self> {
        let v = conn.shm_query_version()?.reply()?;
        if (v.major_version, v.minor_version) < (1, 2) {
            return Err("MIT-SHM 1.2 (fd passing) is not available".into());
        }
        const SIZE: usize = 4096;
        let fd = rustix::fs::memfd_create("glint-spike-shm", rustix::fs::MemfdFlags::CLOEXEC)?;
        rustix::fs::ftruncate(&fd, SIZE as u64)?;
        let file = std::fs::File::from(fd);
        let map = Self::map(&file, SIZE)?;
        let seg = conn.generate_id()?;
        conn.shm_attach_fd(seg, std::os::fd::OwnedFd::from(file), false)?;
        conn.sync()?;
        Ok(Self {
            conn,
            map,
            seg,
            drawable,
            x,
            y,
        })
    }

    #[allow(unsafe_code)]
    fn map(file: &std::fs::File, len: usize) -> Res<memmap2::Mmap> {
        // SAFETY: the memfd was just created by this process and is shared only with the
        // X server. Nothing truncates it while mapped, and it is read only after the
        // server's ShmGetImage reply, i.e. after the server finished writing into it.
        Ok(unsafe { memmap2::MmapOptions::new().len(len).map(file)? })
    }
}

impl Capture for ShmCapture {
    fn grab(&mut self) -> Res<u32> {
        let _reply = self
            .conn
            .shm_get_image(
                self.drawable,
                self.x,
                self.y,
                PATCH,
                PATCH,
                !0,
                ImageFormat::Z_PIXMAP.into(),
                self.seg,
                0,
            )?
            .reply()?;
        Ok(center_pixel(&self.map))
    }
}

// ---------------------------------------------------------------- arguments

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Root,
    Window,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method {
    X11rb,
    Raw,
    Shm,
    /// GetInputFocus instead of GetImage (control load for `tax`).
    Focus,
}

struct Args {
    mode: String,
    target: Target,
    method: Method,
    count: usize,
    warmup: usize,
    rate: Option<u32>, // None = max
    secs: u64,
    label: String,
}

fn parse_args() -> Res<Args> {
    let mut it = std::env::args().skip(1);
    let mode = it.next().unwrap_or_else(|| "help".into());
    let mut a = Args {
        mode,
        target: Target::Root,
        method: Method::Raw,
        count: 1_000_000,
        warmup: 10_000,
        rate: Some(0),
        secs: 20,
        label: "-".into(),
    };
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--target" => {
                a.target = match value()?.as_str() {
                    "root" => Target::Root,
                    "window" => Target::Window,
                    other => return Err(format!("unknown target {other}").into()),
                }
            }
            "--method" => {
                a.method = match value()?.as_str() {
                    "x11rb" => Method::X11rb,
                    "raw" => Method::Raw,
                    "shm" => Method::Shm,
                    "focus" => Method::Focus,
                    other => return Err(format!("unknown method {other}").into()),
                }
            }
            "--count" => a.count = value()?.parse()?,
            "--warmup" => a.warmup = value()?.parse()?,
            "--rate" => {
                let v = value()?;
                a.rate = if v == "max" { None } else { Some(v.parse()?) };
            }
            "--secs" => a.secs = value()?.parse()?,
            "--label" => a.label = value()?,
            other => return Err(format!("unknown flag {other}").into()),
        }
    }
    Ok(a)
}

// ---------------------------------------------------------------- statistics

struct Stats {
    n: usize,
    p50: u32,
    p90: u32,
    p99: u32,
    p999: u32,
    p9999: u32,
    max: u32,
    mean: f64,
    over_1ms: usize,
    over_10ms: usize,
}

fn stats(lat: &mut [u32]) -> Stats {
    lat.sort_unstable();
    let n = lat.len();
    let at = |q: f64| lat[((n as f64 * q) as usize).min(n - 1)];
    let sum: u64 = lat.iter().map(|&v| u64::from(v)).sum();
    Stats {
        n,
        p50: at(0.50),
        p90: at(0.90),
        p99: at(0.99),
        p999: at(0.999),
        p9999: at(0.9999),
        max: lat[n - 1],
        mean: sum as f64 / n as f64,
        over_1ms: lat.iter().filter(|&&v| v > 1_000_000).count(),
        over_10ms: lat.iter().filter(|&&v| v > 10_000_000).count(),
    }
}

fn us(ns: u32) -> f64 {
    f64::from(ns) / 1000.0
}

fn print_stats(what: &str, s: &Stats) {
    println!(
        "  {what}: n {} · p50 {:.1} · p90 {:.1} · p99 {:.1} · p99.9 {:.1} · p99.99 {:.1} · max {:.1} µs · mean {:.1} µs · >1 ms {} · >10 ms {}",
        s.n,
        us(s.p50),
        us(s.p90),
        us(s.p99),
        us(s.p999),
        us(s.p9999),
        us(s.max),
        s.mean / 1000.0,
        s.over_1ms,
        s.over_10ms
    );
}

fn elapsed_ns(t0: Instant) -> u32 {
    u32::try_from(t0.elapsed().as_nanos()).unwrap_or(u32::MAX)
}

/// Sleep until about 60 µs before `deadline`, then spin (PLAN §5.6).
fn pace_until(deadline: Instant) {
    let now = Instant::now();
    if deadline <= now {
        return;
    }
    let left = deadline - now;
    if left > Duration::from_micros(80) {
        std::thread::sleep(left - Duration::from_micros(60));
    }
    while Instant::now() < deadline {
        std::hint::spin_loop();
    }
}

// ---------------------------------------------------------------- modes

fn open_capture(method: Method, drawable: Window, x: i16, y: i16) -> Res<Box<dyn Capture>> {
    Ok(match method {
        Method::X11rb => Box::new(X11rbCapture {
            conn: x11rb::connect(None)?.0,
            drawable,
            x,
            y,
        }),
        Method::Raw => Box::new(RawCapture::new(RawConn::connect()?, drawable, x, y)),
        Method::Shm => Box::new(ShmCapture::new(x11rb::connect(None)?.0, drawable, x, y)?),
        Method::Focus => Box::new(RawFocus {
            conn: RawConn::connect()?,
            data: [0; 64],
        }),
    })
}

fn bench(a: &Args) -> Res<()> {
    let tw = TestWindow::open()?;
    let (drawable, x, y) = tw.spot(a.target);
    let mut cap = open_capture(a.method, drawable, x, y)?;
    for _ in 0..a.warmup {
        cap.grab()?;
    }
    let pixel = cap.grab()?;

    let mut lat = vec![0u32; a.count];
    let allocs_before = ALLOCS.load(Ordering::Relaxed);
    let started = Instant::now();
    for slot in lat.iter_mut() {
        let t0 = Instant::now();
        cap.grab()?;
        *slot = elapsed_ns(t0);
    }
    let total = started.elapsed();
    let allocs = ALLOCS.load(Ordering::Relaxed) - allocs_before;

    let s = stats(&mut lat);
    let rate = a.count as f64 / total.as_secs_f64();
    let per = allocs as f64 / a.count as f64;
    println!("bench target={:?} method={:?} label={}", a.target, a.method, a.label);
    print_stats("round trip", &s);
    println!(
        "  rate {rate:.0} checks/s · heap allocations per check {per:.2} · centre pixel #{pixel:06X} (expected #{COLOR:06X}) {}",
        if pixel == COLOR { "OK" } else { "MISMATCH" }
    );
    println!(
        "RESULT\tbench\t{}\t{:?}\t{:?}\t{}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{rate:.0}\t{per:.2}\t{}\t{}\t{}",
        a.label,
        a.target,
        a.method,
        s.n,
        us(s.p50),
        us(s.p90),
        us(s.p99),
        us(s.p999),
        us(s.p9999),
        us(s.max),
        s.mean / 1000.0,
        s.over_1ms,
        s.over_10ms,
        pixel == COLOR
    );
    Ok(())
}

fn tax(a: &Args) -> Res<()> {
    let tw = TestWindow::open()?;
    let (drawable, x, y) = tw.spot(a.target);
    let stop = Arc::new(AtomicBool::new(false));

    let load = if a.rate == Some(0) {
        None
    } else {
        let stop = Arc::clone(&stop);
        let (method, rate) = (a.method, a.rate);
        let max_samples = 2_000_000;
        Some(std::thread::spawn(move || -> Res<(u64, Duration, Vec<u32>, u64)> {
            let mut cap = open_capture(method, drawable, x, y)?;
            let period = rate.map(|r| Duration::from_nanos(1_000_000_000 / u64::from(r)));
            let mut lat = Vec::with_capacity(max_samples);
            let (mut n, mut stalls) = (0u64, 0u64);
            let started = Instant::now();
            let mut next = started;
            while !stop.load(Ordering::Relaxed) {
                let t0 = Instant::now();
                cap.grab()?;
                if lat.len() < max_samples {
                    lat.push(elapsed_ns(t0));
                }
                n += 1;
                if let Some(p) = period {
                    next += p;
                    let now = Instant::now();
                    if now > next + p * 4 {
                        stalls += 1;
                        next = now;
                    }
                    pace_until(next);
                }
            }
            Ok((n, started.elapsed(), lat, stalls))
        }))
    };

    // The "other client": GetInputFocus once per millisecond.
    let (probe, _) = x11rb::connect(None)?;
    std::thread::sleep(Duration::from_millis(500));
    let deadline = Instant::now() + Duration::from_secs(a.secs);
    let mut lat = Vec::with_capacity(usize::try_from(a.secs * 1100)?);
    let mut next = Instant::now();
    while Instant::now() < deadline {
        let t0 = Instant::now();
        probe.get_input_focus()?.reply()?;
        lat.push(elapsed_ns(t0));
        next += Duration::from_millis(1);
        pace_until(next);
    }
    stop.store(true, Ordering::Relaxed);

    let rate_label = a.rate.map_or_else(|| "max".to_string(), |r| r.to_string());
    println!(
        "tax rate={rate_label} target={:?} method={:?} label={}",
        a.target, a.method, a.label
    );
    let s = stats(&mut lat);
    print_stats("other client's GetInputFocus", &s);
    let mut load_cols = String::from("0\t0\t0\t0");
    if let Some(h) = load {
        let (n, dur, mut llat, stalls) = h.join().map_err(|_| "load thread panicked")??;
        let achieved = n as f64 / dur.as_secs_f64();
        let ls = stats(&mut llat);
        println!("  load: achieved {achieved:.0} checks/s · stalls {stalls}");
        print_stats("load GetImage", &ls);
        load_cols = format!("{achieved:.0}\t{:.1}\t{:.1}\t{stalls}", us(ls.p50), us(ls.p99));
    }
    println!(
        "RESULT\ttax\t{}\t{rate_label}\t{:?}/{:?}\t{}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{load_cols}",
        a.label,
        a.target,
        a.method,
        s.n,
        us(s.p50),
        us(s.p90),
        us(s.p99),
        us(s.p999),
        us(s.max)
    );
    Ok(())
}

/// A GetImage (or GetInputFocus) loop for `secs`, tight or paced with `--rate N`: when do
/// round trips above 300 µs happen, and how far apart are they? Spacing of one refresh period
/// means a per-frame cause. Every stall above 5 ms is also listed with its time, to see whether
/// the big ones are periodic.
fn stalls(a: &Args) -> Res<()> {
    // GetInputFocus needs no window, so a long focus run shows nothing on screen.
    let tw = if matches!(a.method, Method::Focus) {
        None
    } else {
        Some(TestWindow::open()?)
    };
    let (drawable, x, y) = tw.as_ref().map_or((0, 0, 0), |tw| tw.spot(Target::Root));
    let mut cap = open_capture(a.method, drawable, x, y)?;
    let mut events: Vec<(u64, u32)> = Vec::with_capacity(100_000);
    let started = Instant::now();
    let end = Duration::from_secs(a.secs);
    let period = a
        .rate
        .filter(|&r| r > 0)
        .map(|r| Duration::from_nanos(1_000_000_000 / u64::from(r)));
    let mut next = started;
    let mut checks = 0u64;
    while started.elapsed() < end {
        if let Some(p) = period {
            next += p;
            if next < Instant::now() {
                next = Instant::now();
            }
            pace_until(next);
        }
        let t0 = Instant::now();
        cap.grab()?;
        let rtt = elapsed_ns(t0);
        checks += 1;
        if rtt > 300_000 && events.len() < events.capacity() {
            events.push((u64::try_from((t0 - started).as_nanos())?, rtt));
        }
    }
    println!(
        "stalls method={:?} rate={:?} label={} · {checks} checks in {} s",
        a.method, a.rate, a.label, a.secs
    );
    for &(at, rtt) in events.iter().filter(|e| e.1 > 5_000_000) {
        println!(
            "  big stall at {:8.3} s: {:5.1} ms",
            at as f64 / 1e9,
            f64::from(rtt) / 1e6
        );
    }
    if events.len() < 2 {
        println!("  {} stall(s) above 300 µs", events.len());
        return Ok(());
    }
    let mut durations: Vec<u32> = events.iter().map(|e| e.1).collect();
    let mut gaps: Vec<u32> = events
        .windows(2)
        .map(|w| u32::try_from(w[1].0 - w[0].0).unwrap_or(u32::MAX))
        .collect();
    let per_s = events.len() as f64 / a.secs as f64;
    println!("  {} stalls above 300 µs ({per_s:.1} per second)", events.len());
    print_stats("stall length", &stats(&mut durations));
    let near = |g: u32, p: f64| {
        let k = (f64::from(g) / p).round().max(1.0);
        (f64::from(g) - k * p).abs() < 300_000.0
    };
    let frame = 1e9 / 60.0;
    let locked = gaps.iter().filter(|&&g| near(g, frame)).count();
    print_stats("gap between stall starts", &stats(&mut gaps));
    println!(
        "  gaps within ±0.3 ms of a multiple of 16.67 ms (60 Hz): {locked} of {} ({:.0}%)",
        gaps.len(),
        100.0 * locked as f64 / gaps.len() as f64
    );
    Ok(())
}

fn info() -> Res<()> {
    let (conn, screen_num) = x11rb::connect(None)?;
    let setup = conn.setup();
    let screen = &setup.roots[screen_num];
    println!(
        "server: {} (release {}), protocol {}.{}",
        String::from_utf8_lossy(&setup.vendor),
        setup.release_number,
        setup.protocol_major_version,
        setup.protocol_minor_version
    );
    println!(
        "image byte order: {:?} · bitmap bit order: {:?}",
        setup.image_byte_order, setup.bitmap_format_bit_order
    );
    for f in &setup.pixmap_formats {
        println!(
            "pixmap format: depth {} · {} bpp · scanline pad {}",
            f.depth, f.bits_per_pixel, f.scanline_pad
        );
    }
    println!(
        "screen {screen_num}: {}x{} px · root depth {} · root visual {:#x}",
        screen.width_in_pixels, screen.height_in_pixels, screen.root_depth, screen.root_visual
    );
    for d in &screen.allowed_depths {
        for v in d.visuals.iter().filter(|v| v.visual_id == screen.root_visual) {
            println!(
                "root visual: class {:?} · {} bits/rgb · masks R {:#08x} G {:#08x} B {:#08x}",
                v.class, v.bits_per_rgb_value, v.red_mask, v.green_mask, v.blue_mask
            );
        }
    }

    let present = |name: &str| -> Res<bool> { Ok(conn.query_extension(name.as_bytes())?.reply()?.present) };
    let xtest = conn.xtest_get_version(2, 2)?.reply()?;
    println!("XTEST {}.{}", xtest.major_version, xtest.minor_version);
    let shm = conn.shm_query_version()?.reply()?;
    println!(
        "MIT-SHM {}.{} (shared pixmaps: {})",
        shm.major_version, shm.minor_version, shm.shared_pixmaps
    );
    let randr = conn.randr_query_version(1, 6)?.reply()?;
    println!("RANDR {}.{}", randr.major_version, randr.minor_version);
    let xi = conn.xinput_xi_query_version(2, 4)?.reply()?;
    println!("XInputExtension {}.{}", xi.major_version, xi.minor_version);
    let damage = conn.damage_query_version(1, 1)?.reply()?;
    println!("DAMAGE {}.{}", damage.major_version, damage.minor_version);
    let composite = conn.composite_query_version(0, 4)?.reply()?;
    println!("Composite {}.{}", composite.major_version, composite.minor_version);
    let xfixes = conn.xfixes_query_version(6, 0)?.reply()?;
    println!("XFIXES {}.{}", xfixes.major_version, xfixes.minor_version);
    let ss = conn.screensaver_query_version(1, 1)?.reply()?;
    println!(
        "MIT-SCREEN-SAVER {}.{}",
        ss.server_major_version, ss.server_minor_version
    );
    println!("XWAYLAND extension present: {}", present("XWAYLAND")?);

    let monitors = conn.randr_get_monitors(screen.root, true)?.reply()?;
    for m in &monitors.monitors {
        let name = conn.get_atom_name(m.name)?.reply()?;
        println!(
            "monitor {}: {}x{}+{}+{} · primary {} · {} output(s)",
            String::from_utf8_lossy(&name.name),
            m.width,
            m.height,
            m.x,
            m.y,
            m.primary,
            m.outputs.len()
        );
    }

    let cm = conn
        .intern_atom(false, format!("_NET_WM_CM_S{screen_num}").as_bytes())?
        .reply()?
        .atom;
    let owner = conn.get_selection_owner(cm)?.reply()?.owner;
    println!(
        "compositor (_NET_WM_CM_S{screen_num} owner): {}",
        if owner == 0 {
            "none".to_string()
        } else {
            format!("{owner:#x}")
        }
    );
    Ok(())
}

fn main() -> std::process::ExitCode {
    let result = parse_args().and_then(|a| match a.mode.as_str() {
        "info" => info(),
        "bench" => bench(&a),
        "tax" => tax(&a),
        "stalls" => stalls(&a),
        _ => Err("usage: spike-capture info | bench [--target root|window] [--method x11rb|raw|shm] [--count N] [--label T] | tax [--rate 0|500|2000|max] [--secs S] [--method raw|x11rb] [--label T]".into()),
    });
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("spike-capture: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
