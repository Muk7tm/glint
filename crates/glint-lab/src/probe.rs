// SPDX-License-Identifier: GPL-3.0-or-later
//! The stall probe (ADR 0004): a second X client times a round trip every millisecond for the
//! whole run. A round trip longer than [`STALL`] means the server stopped answering for that
//! long; the report sets apart the trials such a stall touched. Round trips before and during
//! watching give the server tax (§5.6).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use glint_x11::env::LocalDisplay;
use x11rb::protocol::xproto::ConnectionExt as _;

use crate::{LabError, x};

/// A round trip longer than this is an X server stall (MEASUREMENTS.md §4 lists stalls the same way).
pub const STALL: Duration = Duration::from_millis(5);
const EVERY: Duration = Duration::from_millis(1);
const BUCKET_US: u64 = 10;
/// 50 ms in 10 µs steps; anything longer lands in the last bucket.
const BUCKETS: usize = 5_000;

/// Round-trip times in 10 µs buckets.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Histogram {
    counts: Vec<u32>,
    n: u64,
}

impl Default for Histogram {
    fn default() -> Self {
        Self {
            counts: vec![0; BUCKETS + 1],
            n: 0,
        }
    }
}

impl Histogram {
    pub fn add(&mut self, d: Duration) {
        let i = usize::try_from(d.as_micros() / u128::from(BUCKET_US))
            .unwrap_or(BUCKETS)
            .min(BUCKETS);
        self.counts[i] += 1;
        self.n += 1;
    }

    pub fn len(&self) -> u64 {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// The `q` quantile, rounded up to its bucket's upper edge.
    pub fn quantile(&self, q: f64) -> Option<Duration> {
        let want = ((self.n as f64 * q).ceil() as u64).clamp(1, self.n.max(1));
        let mut seen = 0;
        for (i, &c) in self.counts.iter().enumerate() {
            seen += u64::from(c);
            if seen >= want && self.n > 0 {
                return Some(Duration::from_micros((i as u64 + 1) * BUCKET_US));
            }
        }
        None
    }
}

/// What the probe saw.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ProbeResult {
    /// Round trips before watching started.
    pub baseline: Histogram,
    /// Round trips while watching.
    pub loaded: Histogram,
    /// Round trips longer than [`STALL`]: when each went out and when it was answered.
    pub stalls: Vec<(Instant, Instant)>,
}

pub struct Probe {
    stop: Arc<AtomicBool>,
    loaded: Arc<AtomicBool>,
    thread: Option<JoinHandle<ProbeResult>>,
}

impl Probe {
    pub fn start(display: LocalDisplay) -> Result<Self, LabError> {
        let conn = glint_x11::conn::open(display).map_err(x)?;
        let stop = Arc::new(AtomicBool::new(false));
        let loaded = Arc::new(AtomicBool::new(false));
        let (quit, watching) = (Arc::clone(&stop), Arc::clone(&loaded));
        let thread = std::thread::Builder::new()
            .name("lab-probe".into())
            .spawn(move || {
                let mut out = ProbeResult::default();
                while !quit.load(Ordering::Acquire) {
                    let sent = Instant::now();
                    let Ok(cookie) = conn.get_input_focus() else { break };
                    if cookie.reply().is_err() {
                        break;
                    }
                    let answered = Instant::now();
                    let rtt = answered - sent;
                    if watching.load(Ordering::Relaxed) {
                        out.loaded.add(rtt);
                    } else {
                        out.baseline.add(rtt);
                    }
                    if rtt > STALL {
                        out.stalls.push((sent, answered));
                    }
                    if let Some(rest) = (sent + EVERY).checked_duration_since(Instant::now()) {
                        std::thread::sleep(rest);
                    }
                }
                out
            })
            .map_err(x)?;
        Ok(Self {
            stop,
            loaded,
            thread: Some(thread),
        })
    }

    /// From now on, round trips count as taken while watching.
    pub fn watching(&self) {
        self.loaded.store(true, Ordering::Relaxed);
    }

    pub fn finish(mut self) -> ProbeResult {
        self.stop.store(true, Ordering::Release);
        self.thread.take().and_then(|t| t.join().ok()).unwrap_or_default()
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles() {
        let mut h = Histogram::default();
        assert_eq!(h.quantile(0.5), None);
        for us in 1..=100u64 {
            h.add(Duration::from_micros(us * 10 - 5));
        }
        h.add(Duration::from_secs(1)); // past the last bucket
        assert_eq!(h.len(), 101);
        assert_eq!(h.quantile(0.5), Some(Duration::from_micros(510)));
        assert_eq!(h.quantile(0.99), Some(Duration::from_micros(1000)));
        assert_eq!(h.quantile(1.0), Some(Duration::from_micros(50_010)));
    }
}
