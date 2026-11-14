// SPDX-License-Identifier: MIT
//! The telemetry thread (PLAN §4.2, §17). It drains the hot thread's event ring, keeps the live
//! statistics, and passes on the events the controller must act on. The hot thread never waits
//! for it: the ring is wait-free, and a full ring drops records and counts them.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::hot::{HotEvent, HotShared, StopReason};

/// How many recent checks the latency percentiles cover.
const WINDOW: usize = 4096;

/// What the controller hears about from the hot thread.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Notice {
    /// A click went out; `latency` is from the capture's return to the press being written.
    Fired { count: u64, latency: Duration },
    /// The hot thread stopped watching on its own.
    Stopped(StopReason),
    /// Blind time may have hidden a new appearance (ADR 0007).
    MaybeMissed,
    /// The hot thread has exited.
    Exited,
    /// DAMAGE proved unreliable and the hot thread went back to polling (§5.2).
    DamageUnreliable,
    /// For trace subscribers (`glint-lab`), built only while one is connected.
    Trace(TraceNotice),
}

/// The trace stream's entries, before the controller turns their times into nanoseconds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TraceNotice {
    Fired {
        request: Instant,
        capture: Instant,
        decided: Instant,
        sent: Instant,
    },
    MaybeMissed {
        at: Instant,
    },
    /// No check came back between `from` and `to`, longer than the check gap.
    Blind {
        from: Instant,
        to: Instant,
    },
    Stopped {
        at: Instant,
        reason: StopReason,
    },
    /// The hot thread's records between `from` and `to` were dropped (the ring was full).
    Lost {
        from: Instant,
        to: Instant,
    },
}

/// The engine's live statistics (§17).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub checks: u64,
    pub checks_per_sec: f64,
    /// Capture round trip, microseconds.
    pub capture_p50_us: f64,
    pub capture_p99_us: f64,
    pub capture_max_us: f64,
    /// From the capture's return to the press being written, microseconds.
    pub decide_p50_us: f64,
    pub decide_max_us: f64,
    pub clicks: u64,
    pub suppressed: u64,
    pub maybe_missed: u64,
    pub stalls: u64,
    pub worst_stall_ms: f64,
    pub dropped: u64,
    pub input_errors: u64,
}

/// Keeps the statistics up to date from the event stream.
pub struct Collector {
    stats: Stats,
    capture: VecDeque<u32>,
    decide: VecDeque<u32>,
    rate_window: VecDeque<Instant>,
    /// When the last check came back.
    last_cap: Option<Instant>,
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

impl Collector {
    pub fn new() -> Self {
        Self {
            stats: Stats::default(),
            capture: VecDeque::with_capacity(WINDOW),
            decide: VecDeque::with_capacity(WINDOW),
            rate_window: VecDeque::new(),
            last_cap: None,
        }
    }

    /// The trace entry for `event`, if it makes one. `gap` is the trigger's check gap: a longer
    /// wait between two checks coming back is blind time (ADR 0007). Call it before
    /// [`record`](Self::record) for the same event.
    pub fn trace(&self, event: &HotEvent, gap: Duration) -> Option<TraceNotice> {
        match *event {
            HotEvent::Check { t_cap, .. } => self
                .last_cap
                .filter(|&from| t_cap.saturating_duration_since(from) > gap)
                .map(|from| TraceNotice::Blind { from, to: t_cap }),
            HotEvent::Fired {
                t_req,
                t_cap,
                t_dec,
                t_sent,
            } => Some(TraceNotice::Fired {
                request: t_req,
                capture: t_cap,
                decided: t_dec,
                sent: t_sent,
            }),
            HotEvent::MaybeMissed { at } => Some(TraceNotice::MaybeMissed { at }),
            HotEvent::Stopped { at, reason } => Some(TraceNotice::Stopped { at, reason }),
            HotEvent::Lost { from, to } => Some(TraceNotice::Lost { from, to }),
            _ => None,
        }
    }

    /// Records one event and returns what the controller should hear about it, if anything.
    pub fn record(&mut self, event: HotEvent) -> Option<Notice> {
        match event {
            HotEvent::Check { t_req, t_cap, .. } => {
                self.last_cap = Some(t_cap);
                self.stats.checks += 1;
                keep(&mut self.capture, micros(t_cap - t_req));
                self.rate_window.push_back(t_cap);
                while self
                    .rate_window
                    .front()
                    .is_some_and(|&t| t_cap - t > Duration::from_secs(1))
                {
                    self.rate_window.pop_front();
                }
                None
            }
            HotEvent::Fired { t_cap, t_sent, .. } => {
                self.stats.clicks += 1;
                keep(&mut self.decide, micros(t_sent - t_cap));
                Some(Notice::Fired {
                    count: self.stats.clicks,
                    latency: t_sent - t_cap,
                })
            }
            HotEvent::Suppressed { .. } => {
                self.stats.suppressed += 1;
                None
            }
            HotEvent::MaybeMissed { .. } => {
                self.stats.maybe_missed += 1;
                Some(Notice::MaybeMissed)
            }
            HotEvent::Stall { late, .. } => {
                self.stats.stalls += 1;
                self.stats.worst_stall_ms = self.stats.worst_stall_ms.max(late.as_secs_f64() * 1e3);
                None
            }
            HotEvent::Stopped { reason, .. } => Some(Notice::Stopped(reason)),
            HotEvent::InputError { .. } => {
                self.stats.input_errors += 1;
                None
            }
            HotEvent::Exited => Some(Notice::Exited),
            HotEvent::DamageUnreliable { .. } => Some(Notice::DamageUnreliable),
            // The checks in the gap are unknown, not missing: the next one starts afresh rather
            // than reporting the whole gap as blind time.
            HotEvent::Lost { .. } => {
                self.last_cap = None;
                None
            }
        }
    }

    /// The statistics now, with `dropped` taken from the hot thread's counter.
    pub fn snapshot(&mut self, dropped: u64) -> Stats {
        let (p50, p99, max) = percentiles(&self.capture);
        let (d50, _, dmax) = percentiles(&self.decide);
        self.stats.capture_p50_us = p50;
        self.stats.capture_p99_us = p99;
        self.stats.capture_max_us = max;
        self.stats.decide_p50_us = d50;
        self.stats.decide_max_us = dmax;
        self.stats.checks_per_sec = self.rate_window.len() as f64;
        self.stats.dropped = dropped;
        self.stats.clone()
    }
}

fn micros(d: Duration) -> u32 {
    u32::try_from(d.as_micros()).unwrap_or(u32::MAX)
}

fn keep(window: &mut VecDeque<u32>, v: u32) {
    if window.len() == WINDOW {
        window.pop_front();
    }
    window.push_back(v);
}

fn percentiles(window: &VecDeque<u32>) -> (f64, f64, f64) {
    if window.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let mut v: Vec<u32> = window.iter().copied().collect();
    v.sort_unstable();
    let at = |q: f64| f64::from(v[((v.len() as f64 * q) as usize).min(v.len() - 1)]);
    (at(0.5), at(0.99), f64::from(v[v.len() - 1]))
}

/// Runs the telemetry thread until the hot thread exits: drains the ring every few
/// milliseconds, refreshes `stats`, and calls `notify` for every [`Notice`].
pub fn run(
    mut events: rtrb::Consumer<HotEvent>,
    shared: Arc<HotShared>,
    stats: Arc<Mutex<Stats>>,
    mut notify: impl FnMut(Notice),
) {
    let mut collector = Collector::new();
    let mut last_snapshot = Instant::now();
    loop {
        let mut exited = false;
        let tracing = shared.tracing.load(Ordering::Relaxed);
        let gap = shared.profile.load().trigger.max_check_gap;
        while let Ok(event) = events.pop() {
            if tracing && let Some(t) = collector.trace(&event, gap) {
                notify(Notice::Trace(t));
            }
            if let Some(n) = collector.record(event) {
                exited |= n == Notice::Exited;
                notify(n);
            }
        }
        if exited || last_snapshot.elapsed() >= Duration::from_millis(100) {
            let snap = collector.snapshot(shared.dropped.load(Ordering::Relaxed));
            if let Ok(mut s) = stats.lock() {
                *s = snap;
            }
            last_snapshot = Instant::now();
        }
        if exited || (events.is_abandoned() && events.slots() == 0) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
mod tests {
    use glint_core::Verdict;

    use super::*;

    #[test]
    fn statistics_from_events() {
        let mut c = Collector::new();
        let t0 = Instant::now();
        for i in 0..100u64 {
            let t_req = t0 + Duration::from_micros(500 * i);
            let t_cap = t_req + Duration::from_micros(if i == 99 { 1500 } else { 20 });
            assert_eq!(
                c.record(HotEvent::Check {
                    t_req,
                    t_cap,
                    verdict: Verdict::Absent
                }),
                None
            );
        }
        let fire = HotEvent::Fired {
            t_req: t0 - Duration::from_micros(20),
            t_cap: t0,
            t_dec: t0 + Duration::from_micros(2),
            t_sent: t0 + Duration::from_micros(7),
        };
        assert_eq!(
            c.trace(&fire, Duration::from_millis(2)),
            Some(TraceNotice::Fired {
                request: t0 - Duration::from_micros(20),
                capture: t0,
                decided: t0 + Duration::from_micros(2),
                sent: t0 + Duration::from_micros(7),
            })
        );
        let fired = c.record(fire);
        assert_eq!(
            fired,
            Some(Notice::Fired {
                count: 1,
                latency: Duration::from_micros(7)
            })
        );
        c.record(HotEvent::Stall {
            at: t0,
            late: Duration::from_millis(12),
        });
        c.record(HotEvent::Suppressed { at: t0 });
        assert_eq!(c.record(HotEvent::MaybeMissed { at: t0 }), Some(Notice::MaybeMissed));
        assert_eq!(
            c.record(HotEvent::Stopped {
                at: t0,
                reason: StopReason::SafetyStop
            }),
            Some(Notice::Stopped(StopReason::SafetyStop))
        );
        let s = c.snapshot(3);
        assert_eq!(
            (s.checks, s.clicks, s.suppressed, s.maybe_missed, s.stalls, s.dropped),
            (100, 1, 1, 1, 1, 3)
        );
        assert_eq!(
            (s.capture_p50_us, s.capture_max_us, s.worst_stall_ms),
            (20.0, 1500.0, 12.0)
        );
        assert_eq!(s.decide_p50_us, 7.0);
        assert_eq!(s.checks_per_sec, 100.0); // 100 checks within the last second
    }

    #[test]
    fn blind_gaps_in_the_trace() {
        let mut c = Collector::new();
        let t0 = Instant::now();
        let gap = Duration::from_millis(2);
        let check = |ms: u64| HotEvent::Check {
            t_req: t0 + Duration::from_millis(ms),
            t_cap: t0 + Duration::from_millis(ms) + Duration::from_micros(20),
            verdict: Verdict::Absent,
        };
        let mut blind = Vec::new();
        for ms in [0, 1, 2, 9, 10, 11] {
            let e = check(ms);
            blind.extend(c.trace(&e, gap));
            c.record(e);
        }
        // Only the 7 ms wait between the checks at 2 and 9 ms is longer than the gap.
        let at = |ms: u64| t0 + Duration::from_millis(ms) + Duration::from_micros(20);
        assert_eq!(blind, [TraceNotice::Blind { from: at(2), to: at(9) }]);
        assert_eq!(
            c.trace(
                &HotEvent::Stopped {
                    at: t0,
                    reason: StopReason::SafetyStop
                },
                gap
            ),
            Some(TraceNotice::Stopped {
                at: t0,
                reason: StopReason::SafetyStop
            })
        );
        assert_eq!(c.trace(&HotEvent::Suppressed { at: t0 }, gap), None);
    }

    #[test]
    fn records_the_ring_dropped_are_unknown_not_blind() {
        let mut c = Collector::new();
        let t0 = Instant::now();
        let gap = Duration::from_millis(2);
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let check = |ms: u64| HotEvent::Check {
            t_req: at(ms),
            t_cap: at(ms),
            verdict: Verdict::Absent,
        };
        let lost = HotEvent::Lost {
            from: at(1),
            to: at(30),
        };
        let mut trace = Vec::new();
        for e in [check(0), lost, check(30), check(31)] {
            trace.extend(c.trace(&e, gap));
            c.record(e);
        }
        // The checks from 1 to 30 ms are gone, not late: no blind time across the gap.
        assert_eq!(
            trace,
            [TraceNotice::Lost {
                from: at(1),
                to: at(30)
            }]
        );
    }
}
