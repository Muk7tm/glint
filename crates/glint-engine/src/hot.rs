// SPDX-License-Identifier: MIT
//! The hot thread (PLAN §4.2, Appendix A.2): check, classify, decide, click, pace.
//!
//! It owns X connections A (capture) and B (input), the trigger and the injector, and reaches the
//! rest of the engine only through lock-free state: the control word, the compiled profile, the
//! event ring and the live preview. It takes no locks, never logs, does no I/O except X traffic
//! and sleeping, and never allocates or frees memory; debug builds check every iteration with
//! `assert_no_alloc`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use glint_core::pattern::{Rect, SamplePattern};
use glint_core::trigger::{Step, Trigger, TriggerCfg};
use glint_core::{Lut, Verdict};
use glint_x11::capture::{Capture, MAX_PIXELS};
use glint_x11::format::PixelFormat;
use glint_x11::inject::{Action, Injector};

/// How long the hot thread sleeps between looks at the control word while not watching. The
/// controller unparks it after every change, so this only bounds a missed wake-up.
const IDLE_PARK: Duration = Duration::from_millis(50);

/// The events the ring can hold: at 2,000 checks a second, about 8 seconds' worth.
pub const EVENT_RING: usize = 16 * 1024;

/// The control word (§4.2): `[armed:1][gate:1][quit:1][reserved:13][epoch:16][x:16][y:16]`.
///
/// `armed` is set only by a manual Start and cleared by every stop, including the hot thread's
/// own; `gate` says whether clicking is allowed right now (the target window is in front, or
/// *Only click in this window* is off). Splitting the two keeps a focus change from undoing a
/// safety stop (C3).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Control {
    pub armed: bool,
    pub gate: bool,
    pub quit: bool,
    /// Bumped whenever watching must restart cleanly.
    pub epoch: u16,
    /// The capture rectangle's top-left corner, in the capture drawable's coordinates.
    pub x: i16,
    pub y: i16,
}

impl Control {
    pub fn pack(self) -> u64 {
        u64::from(self.armed) << 63
            | u64::from(self.gate) << 62
            | u64::from(self.quit) << 61
            | u64::from(self.epoch) << 32
            | u64::from(self.x as u16) << 16
            | u64::from(self.y as u16)
    }

    pub fn unpack(v: u64) -> Self {
        Self {
            armed: v >> 63 & 1 == 1,
            gate: v >> 62 & 1 == 1,
            quit: v >> 61 & 1 == 1,
            epoch: (v >> 32) as u16,
            x: (v >> 16) as u16 as i16,
            y: v as u16 as i16,
        }
    }

    /// The same control with the epoch bumped: watching restarts cleanly.
    pub fn bumped(self) -> Self {
        Self {
            epoch: self.epoch.wrapping_add(1),
            ..self
        }
    }
}

#[derive(Debug, Default)]
pub struct ControlWord(AtomicU64);

impl ControlWord {
    pub fn load(&self) -> Control {
        Control::unpack(self.0.load(Ordering::Acquire))
    }

    /// Applies `f` atomically and returns the new value.
    pub fn update(&self, f: impl Fn(Control) -> Control) -> Control {
        let mut current = self.0.load(Ordering::Acquire);
        loop {
            let new = f(Control::unpack(current));
            match self
                .0
                .compare_exchange_weak(current, new.pack(), Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return new,
                Err(actual) => current = actual,
            }
        }
    }
}

/// Everything the hot loop needs from a profile, ready to use.
#[derive(Debug)]
pub struct CompiledProfile {
    /// Increases with every profile the controller publishes.
    pub revision: u64,
    pub lut: Arc<Lut>,
    pub pattern: SamplePattern,
    pub trigger: TriggerCfg,
    pub action: Action,
    /// The time between checks; `None` is the Max preset, which doesn't sleep.
    pub period: Option<Duration>,
    /// What to read and how its pixels are laid out: the root, or the target window for the
    /// window-buffer backend (§5.2). `None` keeps what the capture was made with.
    pub target: Option<(u32, PixelFormat)>,
    /// Read through the MIT-SHM segment (`screen-shm`, §5.2).
    pub shm: bool,
    /// Check when DAMAGE reports a change over the spot, with safety checks in between, instead
    /// of on a fixed period (the `damage` backend, §5.2).
    pub damage: bool,
    /// The trigger's check gap for polling, if DAMAGE proves unreliable.
    pub poll_gap: Duration,
}

/// DAMAGE's safety period (§5.2): with no report, a check this long after the last one.
pub const DAMAGE_SAFETY: Duration = Duration::from_millis(10);
/// The trigger's check gap under DAMAGE: a safety period and a half. Between reports nothing over
/// the spot changed, so a gap that long isn't blind (ADR 0018).
pub const DAMAGE_CHECK_GAP: Duration = Duration::from_millis(15);
/// After a report, a second check this much later catches late writes (§5.2).
const DAMAGE_FOLLOW_UP: Duration = Duration::from_millis(1);
/// For this long after a report, a change counts as announced: the report's check and its
/// follow-up.
const DAMAGE_CREDIT: Duration = Duration::from_millis(3);

/// The `damage` backend's state in the hot loop (§5.2).
#[derive(Default)]
struct DamageMode {
    /// The profile asks for it.
    wanted: bool,
    /// It missed three changes within a minute: polling for the rest of the engine's life.
    failed: bool,
    /// Following now: the capture's damage object watches the current spot.
    on: bool,
    /// The last wait ended with a change over the spot.
    woke: bool,
    follow_up: Option<Instant>,
    credit_until: Option<Instant>,
    /// A checksum of the last check's pixels.
    last_sum: Option<u64>,
    /// The last three unannounced changes.
    misses: [Option<Instant>; 3],
    next_miss: usize,
}

impl DamageMode {
    /// A (re)start: follow anew from the next check, if the profile asks for it.
    fn reset(&mut self, wanted: bool) {
        self.wanted = wanted && !self.failed;
        self.on = false;
        self.woke = false;
        self.follow_up = None;
        self.credit_until = None;
        self.last_sum = None;
    }

    /// Not watching: stop following, so reports don't pile up unread.
    fn stop(&mut self, capture: &mut dyn Capture) {
        if self.on {
            let _ = capture.watch_damage(None);
            self.on = false;
        }
        self.last_sum = None;
    }

    /// Watching: follow changes over `rect`, if wanted and possible.
    fn start(&mut self, capture: &mut dyn Capture, rect: Rect) {
        if self.wanted && !self.on {
            self.on = capture.watch_damage(Some(rect)).unwrap_or(false);
        }
    }

    /// After a check of `px` (requested at `t_req`): whether the pixels changed with no report to
    /// announce it, for the third time within a minute. `pending` is a report that came in
    /// during this check's own read.
    fn unreliable(&mut self, px: &[u32], t_req: Instant, pending: bool, now: Instant) -> bool {
        let sum = px.iter().fold(0xCBF2_9CE4_8422_2325u64, |h, &p| {
            (h ^ u64::from(p)).wrapping_mul(0x0000_0100_0000_01B3)
        });
        let changed = self.last_sum.is_some_and(|s| s != sum);
        self.last_sum = Some(sum);
        let announced = self.woke || pending || self.credit_until.is_some_and(|c| t_req <= c);
        if !changed || announced {
            return false;
        }
        self.misses[self.next_miss] = Some(now);
        self.next_miss = (self.next_miss + 1) % self.misses.len();
        self.misses
            .iter()
            .all(|m| m.is_some_and(|m| now.saturating_duration_since(m) <= Duration::from_secs(60)))
    }

    /// Polling from now on.
    fn fail(&mut self, capture: &mut dyn Capture) {
        let _ = capture.watch_damage(None);
        self.failed = true;
        self.wanted = false;
        self.on = false;
    }

    /// Waits for a change over the spot, the follow-up check after one, or the safety period
    /// after the check that came back at `t_cap`.
    fn wait(&mut self, capture: &mut dyn Capture, t_cap: Instant) -> Result<(), glint_x11::capture::CaptureError> {
        let deadline = self.follow_up.take().unwrap_or(t_cap + DAMAGE_SAFETY);
        self.woke = capture.wait_damage(deadline)?;
        if self.woke {
            let now = Instant::now();
            self.follow_up = Some(now + DAMAGE_FOLLOW_UP);
            self.credit_until = Some(now + DAMAGE_CREDIT);
        }
        Ok(())
    }
}

/// What the hot thread shares with the rest of the engine.
pub struct HotShared {
    pub control: ControlWord,
    pub profile: ArcSwap<CompiledProfile>,
    /// The revision of the profile the hot thread is using. The controller frees a profile only
    /// once this has moved past it, so the hot thread never frees memory.
    pub in_use: AtomicU64,
    /// Events dropped because the ring was full.
    pub dropped: AtomicU64,
    /// A client wants the trace stream (`glint-lab`); the telemetry thread builds it only then.
    pub tracing: AtomicBool,
    /// The hot thread's kernel thread id, once it runs: the controller schedules it (§5.6).
    pub tid: AtomicI32,
}

/// Why the hot thread stopped watching on its own.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StopReason {
    /// Too many clicks within a second: a feedback loop (§5.5).
    SafetyStop,
    /// The window being read was hidden or closed (§5.2).
    TargetGone,
    /// Reading the screen failed for another reason.
    CaptureFailed,
    /// The input connection failed.
    InputFailed,
}

/// Fixed-size records from the hot thread to the telemetry thread (§4.2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HotEvent {
    /// One check: when the request went out, when the pixels came back, and what they showed.
    Check {
        t_req: Instant,
        t_cap: Instant,
        verdict: Verdict,
    },
    /// A click: the check that fired was requested at `t_req` and came back at `t_cap`, the
    /// trigger decided at `t_dec`, and the press was written at `t_sent`.
    Fired {
        t_req: Instant,
        t_cap: Instant,
        t_dec: Instant,
        t_sent: Instant,
    },
    Suppressed {
        at: Instant,
    },
    MaybeMissed {
        at: Instant,
    },
    /// A check started this late (more than four periods), and pacing resynchronized.
    Stall {
        at: Instant,
        late: Duration,
    },
    /// The hot thread stopped watching on its own; the control word is already disarmed.
    Stopped {
        at: Instant,
        reason: StopReason,
    },
    /// An X error came back on the input connection.
    InputError {
        code: u8,
        major: u8,
    },
    /// The hot thread has exited (after a quit).
    Exited,
    /// DAMAGE missed three changes within a minute; the hot thread went back to polling (§5.2).
    DamageUnreliable {
        at: Instant,
    },
}

/// What the UI's live swatch shows (§12.3 *Seeing*): the color at the watched spot and the verdict.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Preview {
    pub rgb: u32,
    pub verdict: Option<Verdict>,
    /// Increases with every check.
    pub check: u64,
}

/// The hot thread's resources.
pub struct Hot {
    pub shared: Arc<HotShared>,
    pub capture: Box<dyn Capture>,
    pub injector: Injector,
    pub events: rtrb::Producer<HotEvent>,
    pub preview: triple_buffer::Input<Preview>,
}

enum Flow {
    Continue,
    Park,
    Quit,
}

/// Runs until the control word says quit, then releases input and returns.
pub fn run(mut h: Hot) {
    crate::rt::tight_timer_slack();
    h.shared
        .tid
        .store(rustix::thread::gettid().as_raw_nonzero().get(), Ordering::Release);
    let mut profile = h.shared.profile.load_full();
    let mut trigger = Trigger::new(profile.trigger);
    let mut seen_epoch: Option<u16> = None;
    let mut was_armed = false;
    let mut rect = Rect::new(0, 0, 1, 1);
    let mut px = [0u32; MAX_PIXELS];
    let mut next = Instant::now();
    let mut checks = 0u64;
    let mut damage = DamageMode::default();
    loop {
        let flow = assert_no_alloc::assert_no_alloc(|| {
            let ctl = h.shared.control.load();
            if ctl.quit {
                let _ = h.injector.release_now();
                return Flow::Quit;
            }

            // Restart cleanly whenever the controller changed something (R7, ADR 0007).
            if seen_epoch != Some(ctl.epoch) {
                seen_epoch = Some(ctl.epoch);
                profile = h.shared.profile.load_full();
                h.shared.in_use.store(profile.revision, Ordering::Release);
                let _ = h.injector.release_now();
                h.injector.set_action(profile.action);
                if let Some((drawable, format)) = profile.target {
                    h.capture.set_target(drawable, format);
                }
                h.capture.set_shm(profile.shm);
                trigger.set_cfg(profile.trigger);
                let now = Instant::now();
                match (was_armed, ctl.armed) {
                    (_, false) => trigger.stop(),
                    (false, true) => trigger.start(now),  // a manual Start
                    (true, true) => trigger.restart(now), // moved, profile swapped, back in front
                }
                if ctl.armed && !ctl.gate {
                    trigger.pause();
                }
                was_armed = ctl.armed;
                let (pw, ph) = profile.pattern.size();
                rect = Rect::new(ctl.x.into(), ctl.y.into(), pw, ph);
                next = now;
                damage.reset(profile.damage);
            }
            if !ctl.armed || !ctl.gate {
                damage.stop(&mut *h.capture);
                return Flow::Park;
            }
            damage.start(&mut *h.capture, rect);

            let t_req = Instant::now();
            let area = profile.pattern.area();
            if let Err(e) = h.capture.grab(rect, &mut px[..area]) {
                let reason = if e.is_target_gone() {
                    StopReason::TargetGone
                } else {
                    StopReason::CaptureFailed
                };
                stop_self(&mut h, reason, &mut was_armed);
                return Flow::Continue;
            }
            let t_cap = Instant::now();
            let verdict = profile.pattern.verdict(&profile.lut, &px[..area]);

            // A due release goes out before this check can fire (ADR 0007).
            if h.injector.service(t_cap).is_err() {
                stop_self(&mut h, StopReason::InputFailed, &mut was_armed);
                return Flow::Continue;
            }
            match trigger.step_between(t_req, t_cap, verdict) {
                Step::Fire => {
                    let t_dec = Instant::now();
                    if h.injector.press(t_cap).is_err() {
                        stop_self(&mut h, StopReason::InputFailed, &mut was_armed);
                        return Flow::Continue;
                    }
                    push(
                        &mut h,
                        HotEvent::Fired {
                            t_req,
                            t_cap,
                            t_dec,
                            t_sent: Instant::now(),
                        },
                    );
                }
                Step::Suppressed => push(&mut h, HotEvent::Suppressed { at: t_cap }),
                Step::MaybeMissed => push(&mut h, HotEvent::MaybeMissed { at: t_cap }),
                Step::SafetyStop => stop_self(&mut h, StopReason::SafetyStop, &mut was_armed),
                Step::None => {}
            }
            if let Some(e) = h.injector.take_error() {
                push(
                    &mut h,
                    HotEvent::InputError {
                        code: e.code,
                        major: e.major,
                    },
                );
            }
            checks += 1;
            let rgb = profile.pattern.anchor_pixel(&px[..area]).unwrap_or(px[0]);
            h.preview.write(Preview {
                rgb,
                verdict: Some(verdict),
                check: checks,
            });
            push(&mut h, HotEvent::Check { t_req, t_cap, verdict });

            // DAMAGE (§5.2): three changes no report announced within a minute mean reports
            // can't be trusted here, and polling takes over.
            if damage.on && damage.unreliable(&px[..area], t_req, h.capture.damage_pending(), t_cap) {
                damage.fail(&mut *h.capture);
                let mut cfg = profile.trigger;
                cfg.max_check_gap = profile.poll_gap;
                trigger.set_cfg(cfg);
                push(&mut h, HotEvent::DamageUnreliable { at: t_cap });
                next = Instant::now();
            }
            if damage.on {
                if damage.wait(&mut *h.capture, t_cap).is_err() {
                    stop_self(&mut h, StopReason::CaptureFailed, &mut was_armed);
                }
            } else if let Some(period) = profile.period {
                next += period;
                let now = Instant::now();
                if now > next + period * 4 {
                    push(
                        &mut h,
                        HotEvent::Stall {
                            at: now,
                            late: now - next,
                        },
                    );
                    next = now;
                }
                pace_until(next);
            }
            Flow::Continue
        });
        match flow {
            Flow::Continue => {}
            Flow::Park => std::thread::park_timeout(IDLE_PARK),
            Flow::Quit => break,
        }
    }
    push(&mut h, HotEvent::Exited);
}

fn push(h: &mut Hot, event: HotEvent) {
    if h.events.push(event).is_err() {
        h.shared.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

/// Stops watching from inside the hot thread: release, disarm with a fresh epoch, report.
fn stop_self(h: &mut Hot, reason: StopReason, was_armed: &mut bool) {
    let _ = h.injector.release_now();
    h.shared.control.update(|c| Control {
        armed: false,
        ..c.bumped()
    });
    // Even if a Start lands before the next iteration, it must count as a manual Start.
    *was_armed = false;
    push(
        h,
        HotEvent::Stopped {
            at: Instant::now(),
            reason,
        },
    );
}

/// Sleeps until about 60 µs before `deadline`, then spins (§5.6).
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

/// The controller's side of the hot thread: publishing profiles, arming and gating, shutting down.
pub struct HotHandle {
    shared: Arc<HotShared>,
    thread: Option<JoinHandle<()>>,
    /// Profiles the hot thread may still be using; freed once it has moved past them.
    retired: Vec<Arc<CompiledProfile>>,
    revision: u64,
}

impl HotHandle {
    /// Starts the hot thread with `profile`, stopped. Returns the handle, the event ring's reading
    /// end and the live preview's reading end.
    pub fn spawn(
        capture: Box<dyn Capture>,
        injector: Injector,
        mut profile: CompiledProfile,
    ) -> std::io::Result<(Self, rtrb::Consumer<HotEvent>, triple_buffer::Output<Preview>)> {
        profile.revision = 1;
        let shared = Arc::new(HotShared {
            control: ControlWord::default(),
            profile: ArcSwap::from_pointee(profile),
            in_use: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            tracing: AtomicBool::new(false),
            tid: AtomicI32::new(0),
        });
        let (events, consumer) = rtrb::RingBuffer::new(EVENT_RING);
        let (preview, preview_out) = triple_buffer::TripleBuffer::new(&Preview::default()).split();
        let hot = Hot {
            shared: Arc::clone(&shared),
            capture,
            injector,
            events,
            preview,
        };
        let thread = std::thread::Builder::new()
            .name("glintd-hot".into())
            .spawn(move || run(hot))?;
        Ok((
            Self {
                shared,
                thread: Some(thread),
                retired: Vec::new(),
                revision: 1,
            },
            consumer,
            preview_out,
        ))
    }

    pub fn shared(&self) -> &Arc<HotShared> {
        &self.shared
    }

    /// The hot thread's kernel thread id, waiting up to a second for it to start.
    pub fn tid(&self) -> Option<i32> {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match self.shared.tid.load(Ordering::Acquire) {
                0 if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(1)),
                0 => return None,
                tid => return Some(tid),
            }
        }
    }

    /// The control word right now.
    pub fn control(&self) -> Control {
        self.shared.control.load()
    }

    /// Changes the control word and wakes the hot thread.
    pub fn update(&self, f: impl Fn(Control) -> Control) -> Control {
        let c = self.shared.control.update(f);
        if let Some(t) = &self.thread {
            t.thread().unpark();
        }
        c
    }

    /// Publishes a new profile; the hot thread restarts watching with it (a live edit can't click).
    pub fn set_profile(&mut self, mut profile: CompiledProfile) {
        self.revision += 1;
        profile.revision = self.revision;
        let old = self.shared.profile.swap(Arc::new(profile));
        self.retired.push(old);
        self.update(Control::bumped);
        self.free_retired();
    }

    /// The profile the hot thread is using or about to use.
    pub fn profile(&self) -> Arc<CompiledProfile> {
        self.shared.profile.load_full()
    }

    /// Frees retired profiles the hot thread no longer uses. Call it now and then.
    pub fn free_retired(&mut self) {
        let in_use = self.shared.in_use.load(Ordering::Acquire);
        self.retired.retain(|p| p.revision >= in_use);
    }

    /// Manual Start, watching the rectangle at `origin`.
    pub fn start(&self, origin: (i16, i16), gate: bool) {
        self.update(|c| Control {
            armed: true,
            gate,
            x: origin.0,
            y: origin.1,
            ..c.bumped()
        });
    }

    /// Every stop path except quitting: release, disarm.
    pub fn stop(&self) {
        self.update(|c| Control {
            armed: false,
            ..c.bumped()
        });
    }

    /// Restart watching cleanly, at a new origin (the target moved, monitors changed).
    pub fn restart(&self, origin: (i16, i16)) {
        self.update(|c| Control {
            x: origin.0,
            y: origin.1,
            ..c.bumped()
        });
    }

    /// Whether clicking is allowed right now (the target window is in front).
    pub fn set_gate(&self, gate: bool) {
        if self.control().gate != gate {
            self.update(|c| Control { gate, ..c.bumped() });
        }
    }

    /// Stops the hot thread for good; it releases input before it exits.
    pub fn shutdown(&mut self) {
        self.update(|c| Control { quit: true, ..c });
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for HotHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_fails_after_three_unannounced_changes_within_a_minute() {
        let s = Duration::from_secs;
        let t = Instant::now();
        let (a, b) = ([1u32; 9], [2u32; 9]);
        let mut d = DamageMode::default();
        assert!(!d.unreliable(&a, t, false, t)); // nothing to compare yet
        // Announced changes don't count: by the wait's report, by a report during the read, or
        // within the credit after a report.
        d.woke = true;
        assert!(!d.unreliable(&b, t, false, t));
        d.woke = false;
        assert!(!d.unreliable(&a, t, true, t));
        d.credit_until = Some(t + Duration::from_millis(3));
        assert!(!d.unreliable(&b, t + Duration::from_millis(2), false, t));
        d.credit_until = None;
        // No change, no miss.
        assert!(!d.unreliable(&b, t, false, t + s(1)));
        // Unannounced changes: the third within a minute fails.
        assert!(!d.unreliable(&a, t, false, t + s(1)));
        assert!(!d.unreliable(&b, t, false, t + s(2)));
        assert!(d.unreliable(&a, t, false, t + s(3)));
    }

    #[test]
    fn damage_misses_spread_over_more_than_a_minute_are_tolerated() {
        let t = Instant::now();
        let mut d = DamageMode::default();
        let mut px = [0u32; 9];
        assert!(!d.unreliable(&px, t, false, t));
        for i in 1..=5u32 {
            px[0] = i;
            let now = t + Duration::from_secs(u64::from(i) * 40);
            assert!(!d.unreliable(&px, now, false, now), "miss {i}");
        }
    }

    #[test]
    fn the_control_word_round_trips() {
        for c in [
            Control::default(),
            Control {
                armed: true,
                gate: true,
                quit: false,
                epoch: 65535,
                x: -32768,
                y: 32767,
            },
            Control {
                armed: false,
                gate: true,
                quit: true,
                epoch: 1,
                x: 1919,
                y: -1,
            },
        ] {
            assert_eq!(Control::unpack(c.pack()), c);
        }
        assert_eq!(
            Control {
                epoch: 65535,
                ..Control::default()
            }
            .bumped()
            .epoch,
            0
        );
    }

    #[test]
    fn updates_are_atomic_across_threads() {
        let word = Arc::new(ControlWord::default());
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let w = Arc::clone(&word);
                std::thread::spawn(move || {
                    for _ in 0..1000 {
                        w.update(Control::bumped);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(word.load().epoch, 4000);
    }
}
