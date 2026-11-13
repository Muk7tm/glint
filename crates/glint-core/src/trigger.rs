// SPDX-License-Identifier: GPL-3.0-or-later
//! The trigger state machine (PLAN §5.5, ADR 0007).
//!
//! **Speed on the rising edge:** once armed, the first Enter check fires, with no waiting
//! (unless you turn on confirmation). **Safety on the falling edge:** firing latches, and only an
//! observed absence of at least `rearm_absent` and `rearm_absent_samples` checks re-arms. Every
//! (re)start begins latched, so a color that was already showing can't click. The minimum gap
//! and the safety stop sit behind the latch, so they can suppress a click but never add one.
//!
//! Everything here is plain data with no allocation, so the hot loop can own a [`Trigger`] and a
//! [`Hold`] directly.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::pattern::Verdict;

/// The highest `max_clicks_per_sec` (§5.5), and the size of the fixed fire history.
pub const MAX_CLICKS_PER_SEC_LIMIT: u32 = 100;

/// The shortest [`TriggerCfg::max_check_gap`].
pub const MIN_CHECK_GAP: Duration = Duration::from_millis(2);

const ONE_SECOND: Duration = Duration::from_secs(1);

/// The trigger's settings, in the form the hot loop uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TriggerCfg {
    /// Consecutive Enter checks needed before firing (1 = fire on the first).
    pub confirm_samples: u32,
    /// How long the color must keep matching before firing (zero = off).
    pub confirm_min: Duration,
    /// Observed absence needed to re-arm ("Gone after").
    pub rearm_absent: Duration,
    /// Absent checks needed to re-arm, on top of the time.
    pub rearm_absent_samples: u32,
    /// The floor between two clicks. Use [`TriggerCfg::min_gap`] to raise it above the hold time.
    pub min_fire_interval: Duration,
    /// More clicks than this within one second trip the safety stop (1–100).
    pub max_clicks_per_sec: u32,
    /// A manual Start may fire for a color that is already showing.
    pub fire_if_present_on_start: bool,
    /// A gap between two absent checks counts as observed absence only up to this long. Use
    /// [`TriggerCfg::check_gap`].
    pub max_check_gap: Duration,
}

impl TriggerCfg {
    /// The effective minimum gap between clicks: `min_fire_interval_ms`, raised to the hold time
    /// plus 1 ms, so a click can never come due while the previous one is still held (ADR 0007).
    pub fn min_gap(min_fire_interval: Duration, hold: Duration) -> Duration {
        min_fire_interval.max(hold + Duration::from_millis(1))
    }

    /// The longest gap between checks that still counts as watching (ADR 0007): four check
    /// periods (the stall threshold of §5.6), and never less than [`MIN_CHECK_GAP`]. `None` is
    /// the Max preset, which doesn't pace.
    pub fn check_gap(period: Option<Duration>) -> Duration {
        period.map_or(MIN_CHECK_GAP, |p| p.saturating_mul(4).max(MIN_CHECK_GAP))
    }
}

impl Default for TriggerCfg {
    /// The plan's defaults (§5.5) with the default 10 ms hold and the Balanced preset.
    fn default() -> Self {
        Self {
            confirm_samples: 1,
            confirm_min: Duration::ZERO,
            rearm_absent: Duration::from_millis(40),
            rearm_absent_samples: 2,
            min_fire_interval: Self::min_gap(Duration::from_millis(25), Duration::from_millis(10)),
            max_clicks_per_sec: 15,
            fire_if_present_on_start: false,
            max_check_gap: Self::check_gap(Some(Duration::from_micros(500))),
        }
    }
}

/// What a check led to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    None,
    /// Press now.
    Fire,
    /// The trigger fired, but within the minimum gap of the previous click: the appearance is
    /// consumed and nothing is pressed.
    Suppressed,
    /// The trigger fired one click too many within a second: watching stopped (a feedback loop,
    /// §5.5). Release held input and report it.
    SafetyStop,
}

impl Step {
    /// The trigger fired on this check, whether or not it clicked.
    pub fn is_fire_attempt(self) -> bool {
        self != Self::None
    }
}

/// The phase as reported to clients.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerState {
    /// Not watching. Only a manual Start leaves this state.
    Stopped,
    /// Started, but the target window isn't in front: nothing is decided until it is.
    Paused,
    /// Watching, and the color has been seen absent: the next Enter fires.
    Idle,
    /// Confirmation is on and the color is being confirmed.
    Confirming,
    /// Fired or just (re)started: waiting to see the color absent.
    Latched,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Stopped,
    Paused,
    Idle,
    Confirming { since: Instant, count: u32 },
    Latched(Absence),
}

/// The current run of absent checks while latched.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Absence {
    checks: u32,
    /// Absence time actually observed in this run.
    observed: Duration,
    last: Option<Instant>,
}

/// The times of the last clicks, for the safety stop. Fixed size: never allocates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct FireHistory {
    times: [Option<Instant>; MAX_CLICKS_PER_SEC_LIMIT as usize],
    next: usize,
}

impl FireHistory {
    fn new() -> Self {
        Self {
            times: [None; MAX_CLICKS_PER_SEC_LIMIT as usize],
            next: 0,
        }
    }

    fn push(&mut self, t: Instant) {
        self.times[self.next] = Some(t);
        self.next = (self.next + 1) % self.times.len();
    }

    /// Clicks less than one second before `now`. Exact as long as the limit is at most 100,
    /// because the safety stop never lets more through.
    fn last_second(&self, now: Instant) -> u32 {
        self.times
            .iter()
            .flatten()
            .filter(|&&t| now.saturating_duration_since(t) < ONE_SECOND)
            .count() as u32
    }
}

#[derive(Clone, Debug)]
pub struct Trigger {
    cfg: TriggerCfg,
    phase: Phase,
    last_fire: Option<Instant>,
    history: FireHistory,
}

impl Trigger {
    /// A stopped trigger.
    pub fn new(cfg: TriggerCfg) -> Self {
        Self {
            cfg,
            phase: Phase::Stopped,
            last_fire: None,
            history: FireHistory::new(),
        }
    }

    pub fn cfg(&self) -> &TriggerCfg {
        &self.cfg
    }

    /// Swaps in a new profile's settings. The minimum gap and the safety stop keep their history.
    /// The engine follows this with [`restart`](Self::restart), because a profile swap bumps the
    /// epoch (§4.2).
    pub fn set_cfg(&mut self, cfg: TriggerCfg) {
        self.cfg = cfg;
    }

    /// A manual Start: the Start button, the hotkey, a `start` request or `--start-on-launch`.
    /// Watching begins latched, so the color has to be seen absent before anything can fire (R7,
    /// ADR 0007), unless `fire_if_present_on_start` is on. Does nothing while already started.
    pub fn start(&mut self) {
        if self.phase == Phase::Stopped {
            self.phase = if self.cfg.fire_if_present_on_start {
                Phase::Idle
            } else {
                Phase::Latched(Absence::default())
            };
        }
    }

    /// Watching restarts cleanly: the target moved or was resized, the monitors changed, the
    /// profile was swapped, or the target window came back to the front. It begins latched with a
    /// fresh absence count whatever the color is doing. Does nothing while stopped, so only a
    /// manual Start ever starts watching, including after a safety stop (ADR 0007).
    pub fn restart(&mut self) {
        if self.phase != Phase::Stopped {
            self.phase = Phase::Latched(Absence::default());
        }
    }

    /// The target window left the front (*Only click in this window*): nothing is decided until
    /// [`restart`](Self::restart). Does nothing while stopped.
    pub fn pause(&mut self) {
        if self.phase != Phase::Stopped {
            self.phase = Phase::Paused;
        }
    }

    /// Stop, emergency stop, target hidden or closed, screen locked.
    pub fn stop(&mut self) {
        self.phase = Phase::Stopped;
    }

    pub fn state(&self) -> TriggerState {
        match self.phase {
            Phase::Stopped => TriggerState::Stopped,
            Phase::Paused => TriggerState::Paused,
            Phase::Idle => TriggerState::Idle,
            Phase::Confirming { .. } => TriggerState::Confirming,
            Phase::Latched(_) => TriggerState::Latched,
        }
    }

    /// True while checks are being decided (not stopped or paused).
    pub fn is_watching(&self) -> bool {
        !matches!(self.phase, Phase::Stopped | Phase::Paused)
    }

    /// One check: `now` is when its capture completed.
    pub fn step(&mut self, now: Instant, v: Verdict) -> Step {
        match self.phase {
            Phase::Stopped | Phase::Paused => Step::None,
            Phase::Idle => match v {
                Verdict::Enter if self.cfg.confirm_samples <= 1 && self.cfg.confirm_min.is_zero() => self.fire(now),
                Verdict::Enter => {
                    self.phase = Phase::Confirming { since: now, count: 1 };
                    Step::None
                }
                // Present alone never starts an appearance.
                Verdict::Present | Verdict::Absent => Step::None,
            },
            Phase::Confirming { since, count } => {
                if v != Verdict::Enter {
                    self.phase = Phase::Idle;
                    return Step::None;
                }
                let count = count.saturating_add(1);
                if count >= self.cfg.confirm_samples && now.saturating_duration_since(since) >= self.cfg.confirm_min {
                    self.fire(now)
                } else {
                    self.phase = Phase::Confirming { since, count };
                    Step::None
                }
            }
            Phase::Latched(mut absence) => {
                if v.present() {
                    self.phase = Phase::Latched(Absence::default());
                    return Step::None;
                }
                // Count only absence that was actually observed (§5.5): time between two absent
                // checks counts only up to the check gap, so a stall can't stand in for watching.
                if let Some(last) = absence.last {
                    absence.observed += now.saturating_duration_since(last).min(self.cfg.max_check_gap);
                }
                absence.checks = absence.checks.saturating_add(1);
                absence.last = Some(now);
                self.phase =
                    if absence.checks >= self.cfg.rearm_absent_samples && absence.observed >= self.cfg.rearm_absent {
                        Phase::Idle
                    } else {
                        Phase::Latched(absence)
                    };
                Step::None
            }
        }
    }

    fn fire(&mut self, now: Instant) -> Step {
        // 1. Consume the appearance first, so nothing below can lead to a late click.
        self.phase = Phase::Latched(Absence::default());
        // 2. The absolute floor between clicks.
        if self
            .last_fire
            .is_some_and(|t| now.saturating_duration_since(t) < self.cfg.min_fire_interval)
        {
            return Step::Suppressed;
        }
        // 3. The safety stop for feedback loops.
        if self.history.last_second(now) >= self.cfg.max_clicks_per_sec {
            self.phase = Phase::Stopped;
            return Step::SafetyStop;
        }
        self.history.push(now);
        self.last_fire = Some(now);
        Step::Fire
    }
}

/// When to release the action's input (§6). The press goes out the moment the trigger fires and
/// the release `hold` later. The engine's injector sends the X requests in the order this says;
/// keeping the timing here lets invariant 6 be tested without an X server.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Hold {
    release_at: Option<Instant>,
}

impl Hold {
    /// Records a press at `now`. Returns true if the previous press was still held and must be
    /// released before this one, so presses and releases always alternate. That can't happen
    /// when the engine checks [`due`](Self::due) before acting on a fire, because the trigger's
    /// minimum gap is longer than the hold (ADR 0007).
    pub fn press(&mut self, now: Instant, hold: Duration) -> bool {
        self.release_at.replace(now + hold).is_some()
    }

    /// True, once, when the held input is due for release.
    pub fn due(&mut self, now: Instant) -> bool {
        if self.release_at.is_some_and(|t| now >= t) {
            self.release_at = None;
            true
        } else {
            false
        }
    }

    /// Release now, on every stop path. True if something was held.
    pub fn release(&mut self) -> bool {
        self.release_at.take().is_some()
    }

    pub fn is_held(&self) -> bool {
        self.release_at.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn cfg() -> TriggerCfg {
        TriggerCfg::default()
    }

    /// A trigger that has fired once at `t0` and is latched on a present color.
    fn fired(cfg: TriggerCfg) -> (Trigger, Instant) {
        let mut t = Trigger::new(TriggerCfg {
            fire_if_present_on_start: true,
            ..cfg
        });
        let t0 = Instant::now();
        t.start();
        assert_eq!(t.step(t0, Verdict::Enter), Step::Fire);
        t.set_cfg(cfg);
        (t, t0)
    }

    /// Absent checks every `every` from `from` (exclusive) through `to` (inclusive).
    fn absent(t: &mut Trigger, from: Instant, to: Instant, every: Duration) -> Step {
        let mut now = from;
        let mut last = Step::None;
        while now + every <= to {
            now += every;
            last = t.step(now, Verdict::Absent);
            assert_eq!(last, Step::None);
        }
        last
    }

    #[test]
    fn defaults_follow_the_plan() {
        let c = cfg();
        assert_eq!(
            (c.confirm_samples, c.confirm_min, c.rearm_absent, c.rearm_absent_samples),
            (1, ms(0), ms(40), 2)
        );
        assert_eq!(
            (c.min_fire_interval, c.max_clicks_per_sec, c.fire_if_present_on_start),
            (ms(25), 15, false)
        );
        assert_eq!(c.max_check_gap, ms(2));
    }

    #[test]
    fn a_dropout_shorter_than_rearm_never_refires() {
        let (mut t, t0) = fired(cfg());
        assert_eq!(t.step(t0 + ms(10), Verdict::Absent), Step::None); // absence starts here
        absent(&mut t, t0 + ms(10), t0 + ms(30), ms(1)); // 20 ms observed
        assert_eq!(t.step(t0 + ms(31), Verdict::Enter), Step::None); // same appearance
        assert_eq!(t.state(), TriggerState::Latched);
    }

    #[test]
    fn a_full_absence_rearms_and_the_next_enter_fires_at_once() {
        let (mut t, t0) = fired(cfg());
        assert_eq!(t.step(t0 + ms(5), Verdict::Absent), Step::None);
        absent(&mut t, t0 + ms(5), t0 + ms(45), ms(1)); // 40 ms observed: re-armed
        assert_eq!(t.state(), TriggerState::Idle);
        assert_eq!(t.step(t0 + ms(46), Verdict::Present), Step::None); // Present never starts one
        assert_eq!(t.step(t0 + ms(47), Verdict::Enter), Step::Fire);
    }

    #[test]
    fn rearming_needs_enough_checks_as_well_as_time() {
        let (mut t, t0) = fired(TriggerCfg {
            rearm_absent_samples: 5,
            max_check_gap: ms(100),
            ..cfg()
        });
        for i in 1..=4 {
            assert_eq!(t.step(t0 + ms(20 * i), Verdict::Absent), Step::None);
        }
        assert_eq!(t.state(), TriggerState::Latched); // 60 ms observed, but 4 checks
        assert_eq!(t.step(t0 + ms(100), Verdict::Absent), Step::None);
        assert_eq!(t.state(), TriggerState::Idle);
    }

    #[test]
    fn starting_while_present_waits_for_the_color_to_leave() {
        let mut t = Trigger::new(cfg());
        let t0 = Instant::now();
        t.start();
        assert_eq!(t.step(t0, Verdict::Enter), Step::None);
        assert_eq!(t.step(t0 + ms(1), Verdict::Absent), Step::None);
        absent(&mut t, t0 + ms(1), t0 + ms(41), ms(1)); // 40 ms observed: re-armed
        assert_eq!(t.step(t0 + ms(42), Verdict::Enter), Step::Fire);
    }

    #[test]
    fn every_start_begins_latched_even_if_the_first_check_is_absent() {
        // C1 (ADR 0007): one absent check right after Start is not an observed absence.
        let mut t = Trigger::new(cfg());
        let t0 = Instant::now();
        t.start();
        assert_eq!(t.state(), TriggerState::Latched);
        assert_eq!(t.step(t0, Verdict::Absent), Step::None);
        assert_eq!(t.step(t0 + ms(1), Verdict::Enter), Step::None);
    }

    #[test]
    fn a_restart_during_an_appearance_never_clicks() {
        // The color stays on screen while the window moves; the first capture after the move
        // happens to read a stale frame without it.
        let (mut t, t0) = fired(cfg());
        t.restart();
        assert_eq!(t.step(t0 + ms(5), Verdict::Absent), Step::None);
        assert_eq!(t.step(t0 + ms(6), Verdict::Enter), Step::None);
        assert_eq!(t.state(), TriggerState::Latched);
    }

    #[test]
    fn fire_if_present_on_start_applies_to_a_manual_start_only() {
        // C2 (ADR 0007).
        let c = TriggerCfg {
            fire_if_present_on_start: true,
            ..cfg()
        };
        let mut t = Trigger::new(c);
        let t0 = Instant::now();
        t.start();
        assert_eq!(t.step(t0, Verdict::Enter), Step::Fire);
        absent(&mut t, t0, t0 + ms(60), ms(1));
        t.restart(); // the window came back to the front with the color showing
        assert_eq!(t.step(t0 + ms(61), Verdict::Enter), Step::None);
        // A second Start while started changes nothing.
        t.start();
        assert_eq!(t.step(t0 + ms(62), Verdict::Enter), Step::None);
    }

    #[test]
    fn a_stall_is_not_observed_absence() {
        // ADR 0007: the engine stalls for 50 ms between two absent checks.
        let (mut t, t0) = fired(cfg());
        assert_eq!(t.step(t0 + ms(1), Verdict::Absent), Step::None);
        assert_eq!(t.step(t0 + ms(51), Verdict::Absent), Step::None); // 2 ms credited, not 50
        assert_eq!(t.state(), TriggerState::Latched);
        absent(&mut t, t0 + ms(51), t0 + ms(88), ms(1)); // 39 ms observed
        assert_eq!(t.state(), TriggerState::Latched);
        assert_eq!(t.step(t0 + ms(89), Verdict::Absent), Step::None); // 40 ms
        assert_eq!(t.state(), TriggerState::Idle);
    }

    #[test]
    fn a_stall_between_two_dropouts_cant_fake_a_disappearance() {
        // The color is present except for two short dropouts. A 45 ms stall hides the presence
        // between them, so the trigger sees "absent ... absent" 45 ms apart. Counting the stall
        // as absence (the plan's original sketch) would re-arm and click again.
        let (mut t, t0) = fired(cfg());
        assert_eq!(t.step(t0 + ms(100), Verdict::Absent), Step::None); // first dropout
        assert_eq!(t.step(t0 + ms(145), Verdict::Absent), Step::None); // second dropout after the stall
        assert_eq!(t.step(t0 + ms(146), Verdict::Enter), Step::None);
        assert_eq!(t.state(), TriggerState::Latched);
    }

    #[test]
    fn the_minimum_gap_suppresses_and_consumes_the_appearance() {
        let c = TriggerCfg {
            rearm_absent: ms(0),
            rearm_absent_samples: 1,
            ..cfg()
        };
        let (mut t, t0) = fired(c);
        assert_eq!(t.step(t0 + ms(5), Verdict::Absent), Step::None); // re-armed at once
        assert_eq!(t.step(t0 + ms(10), Verdict::Enter), Step::Suppressed); // 10 ms < 25 ms
        assert_eq!(t.step(t0 + ms(30), Verdict::Enter), Step::None); // still the same appearance
        assert_eq!(t.step(t0 + ms(31), Verdict::Absent), Step::None);
        assert_eq!(t.step(t0 + ms(32), Verdict::Enter), Step::Fire);
    }

    #[test]
    fn the_hold_raises_the_minimum_gap() {
        // C4 (ADR 0007): with a 200 ms hold, a 25 ms gap would let a click come due while the
        // previous one is still held.
        assert_eq!(TriggerCfg::min_gap(ms(25), ms(200)), ms(201));
        assert_eq!(TriggerCfg::min_gap(ms(25), ms(10)), ms(25));
        assert_eq!(TriggerCfg::min_gap(ms(0), ms(0)), ms(1));
    }

    #[test]
    fn the_safety_stop_trips_on_the_sixteenth_click_and_needs_a_manual_start() {
        let c = TriggerCfg {
            rearm_absent: ms(0),
            rearm_absent_samples: 1,
            ..cfg()
        };
        let (mut t, t0) = fired(c); // click 1 at 0 ms
        for k in 1..15 {
            assert_eq!(t.step(t0 + ms(30 * k - 10), Verdict::Absent), Step::None);
            assert_eq!(t.step(t0 + ms(30 * k), Verdict::Enter), Step::Fire, "click {}", k + 1);
        }
        assert_eq!(t.step(t0 + ms(440), Verdict::Absent), Step::None);
        assert_eq!(t.step(t0 + ms(450), Verdict::Enter), Step::SafetyStop); // 15 clicks in the last second
        assert_eq!(t.state(), TriggerState::Stopped);
        // Invariant 5: restarts and pauses don't resume watching; only a manual Start does.
        t.restart();
        t.pause();
        assert_eq!(t.step(t0 + ms(2000), Verdict::Enter), Step::None);
        t.start();
        assert_eq!(t.step(t0 + ms(2001), Verdict::Absent), Step::None);
        assert_eq!(t.step(t0 + ms(2002), Verdict::Enter), Step::Fire);
    }

    #[test]
    fn confirmation_by_count_and_by_time() {
        let mut t = Trigger::new(TriggerCfg {
            confirm_samples: 3,
            fire_if_present_on_start: true,
            ..cfg()
        });
        let t0 = Instant::now();
        t.start();
        assert_eq!(t.step(t0, Verdict::Enter), Step::None);
        assert_eq!(t.state(), TriggerState::Confirming);
        assert_eq!(t.step(t0 + ms(1), Verdict::Enter), Step::None);
        assert_eq!(t.step(t0 + ms(2), Verdict::Present), Step::None); // breaks the run
        assert_eq!(t.state(), TriggerState::Idle);
        assert_eq!(t.step(t0 + ms(3), Verdict::Enter), Step::None);
        assert_eq!(t.step(t0 + ms(4), Verdict::Enter), Step::None);
        assert_eq!(t.step(t0 + ms(5), Verdict::Enter), Step::Fire);

        let mut t = Trigger::new(TriggerCfg {
            confirm_min: ms(7),
            fire_if_present_on_start: true,
            ..cfg()
        });
        t.start();
        assert_eq!(t.step(t0, Verdict::Enter), Step::None);
        assert_eq!(t.step(t0 + ms(6), Verdict::Enter), Step::None);
        assert_eq!(t.step(t0 + ms(7), Verdict::Enter), Step::Fire);
    }

    #[test]
    fn paused_decides_nothing_and_a_restart_resumes_latched() {
        let mut t = Trigger::new(TriggerCfg {
            fire_if_present_on_start: true,
            ..cfg()
        });
        let t0 = Instant::now();
        t.start();
        t.pause();
        assert_eq!(t.state(), TriggerState::Paused);
        assert!(!t.is_watching());
        assert_eq!(t.step(t0, Verdict::Enter), Step::None);
        t.restart(); // back in front
        assert_eq!(t.state(), TriggerState::Latched);
        assert_eq!(t.step(t0 + ms(1), Verdict::Enter), Step::None);
        t.stop();
        t.restart();
        assert_eq!(t.state(), TriggerState::Stopped);
    }

    #[test]
    fn hold_timing() {
        let mut h = Hold::default();
        let t0 = Instant::now();
        assert!(!h.press(t0, ms(10)));
        assert!(h.is_held());
        assert!(!h.due(t0 + ms(9)));
        assert!(h.due(t0 + ms(10)));
        assert!(!h.due(t0 + ms(11)));
        assert!(!h.release());
        assert!(!h.press(t0, ms(0)));
        assert!(h.press(t0, ms(0))); // still held: release first
        assert!(h.release());
        assert!(!h.is_held());
    }

    mod invariants {
        //! The six invariants of PLAN §5.5 over random check streams with random timing, stalls,
        //! starts, restarts, pauses and stops. The trigger and a `Hold` run the way the engine's
        //! hot loop will (a due release before acting on a check; release on every stop path),
        //! and a separate oracle checks the trace.

        use proptest::prelude::*;
        use proptest::strategy::ValueTree;

        use super::*;

        #[derive(Clone, Copy, Debug)]
        enum Ev {
            Check { dt_us: u32, v: Verdict },
            Start,
            Restart,
            Pause,
            Stop,
        }

        #[derive(Clone, Copy, Debug)]
        enum Rec {
            Check {
                t: Instant,
                v: Verdict,
                step: Step,
            },
            Start,
            Restart,
            Pause,
            Stop,
            Press,
            Release,
            /// A press found the previous one still held.
            StaleRelease,
        }

        fn verdict() -> impl Strategy<Value = Verdict> {
            prop_oneof![Just(Verdict::Absent), Just(Verdict::Present), Just(Verdict::Enter)]
        }

        fn segment() -> impl Strategy<Value = Vec<Ev>> {
            prop_oneof![
                // A steady stretch, possibly with one stall in it.
                10 => (verdict(), prop_oneof![1usize..12, 12usize..150], 20u32..2500, 0u32..80_000, 0usize..300)
                    .prop_map(|(v, n, dt_us, stall_us, stall_at)| {
                        (0..n).map(|i| Ev::Check { dt_us: if i == stall_at { stall_us } else { dt_us }, v }).collect()
                    }),
                // Flicker: every check different, with irregular timing.
                5 => prop::collection::vec((verdict(), 20u32..20_000), 1..80)
                    .prop_map(|v| v.into_iter().map(|(v, dt_us)| Ev::Check { dt_us, v }).collect()),
                // Blinking on a fixed rhythm: rapid appearances and feedback loops.
                5 => (1usize..6, 1usize..30, 50u32..3000, 2usize..40).prop_map(|(on, off, dt_us, times)| {
                    let blink = std::iter::repeat_n(Ev::Check { dt_us, v: Verdict::Enter }, on)
                        .chain(std::iter::repeat_n(Ev::Check { dt_us, v: Verdict::Absent }, off));
                    blink.cycle().take((on + off) * times).collect()
                }),
                1 => Just(vec![Ev::Start]),
                1 => Just(vec![Ev::Restart]),
                1 => Just(vec![Ev::Pause]),
                1 => Just(vec![Ev::Stop]),
            ]
        }

        fn config() -> impl Strategy<Value = (TriggerCfg, Duration)> {
            (
                prop_oneof![4 => Just(1u32), 1 => 2u32..=4],
                prop_oneof![4 => Just(0u64), 1 => 1u64..=10],
                0u64..=60,
                1u32..=5,
                0u64..=40,
                0u64..=50,
                1u32..=20,
                any::<bool>(),
                prop_oneof![Just(Some(500u64)), Just(Some(2000)), Just(Some(100)), Just(None)],
            )
                .prop_map(|(cs, cmin, rearm, rs, gap, hold, maxc, fip, period)| {
                    let hold = Duration::from_millis(hold);
                    let cfg = TriggerCfg {
                        confirm_samples: cs,
                        confirm_min: Duration::from_millis(cmin),
                        rearm_absent: Duration::from_millis(rearm),
                        rearm_absent_samples: rs,
                        min_fire_interval: TriggerCfg::min_gap(Duration::from_millis(gap), hold),
                        max_clicks_per_sec: maxc,
                        fire_if_present_on_start: fip,
                        max_check_gap: TriggerCfg::check_gap(period.map(Duration::from_micros)),
                    };
                    (cfg, hold)
                })
        }

        /// Runs the events the way the hot loop will and records everything.
        fn run(cfg: TriggerCfg, hold: Duration, events: &[Ev]) -> Vec<Rec> {
            let mut trigger = Trigger::new(cfg);
            let mut held = Hold::default();
            let mut now = Instant::now();
            let mut trace = Vec::new();
            fn release(held: &mut Hold, trace: &mut Vec<Rec>) {
                if held.release() {
                    trace.push(Rec::Release);
                }
            }
            for &ev in events {
                match ev {
                    Ev::Check { dt_us, v } => {
                        now += Duration::from_micros(u64::from(dt_us));
                        if held.due(now) {
                            trace.push(Rec::Release);
                        }
                        let step = trigger.step(now, v);
                        trace.push(Rec::Check { t: now, v, step });
                        match step {
                            Step::Fire => {
                                if held.press(now, hold) {
                                    trace.push(Rec::StaleRelease);
                                }
                                trace.push(Rec::Press);
                            }
                            Step::SafetyStop => release(&mut held, &mut trace),
                            Step::None | Step::Suppressed => {}
                        }
                    }
                    Ev::Start => {
                        trigger.start();
                        trace.push(Rec::Start);
                    }
                    Ev::Restart => {
                        release(&mut held, &mut trace);
                        trigger.restart();
                        trace.push(Rec::Restart);
                    }
                    Ev::Pause => {
                        release(&mut held, &mut trace);
                        trigger.pause();
                        trace.push(Rec::Pause);
                    }
                    Ev::Stop => {
                        release(&mut held, &mut trace);
                        trigger.stop();
                        trace.push(Rec::Stop);
                    }
                }
            }
            release(&mut held, &mut trace);
            trigger.stop();
            trace
        }

        /// Checks the trace against the invariants, with its own model of "observed absence".
        fn check(cfg: &TriggerCfg, trace: &[Rec]) -> Result<(), TestCaseError> {
            let confirm_off = cfg.confirm_samples <= 1 && cfg.confirm_min.is_zero();
            let mut started = false; // between a manual Start and a stop
            let mut paused = false;
            let mut armed = false; // a qualifying observed absence since the last (re)start or fire
            let mut free = false; // a manual Start with fire_if_present_on_start: no absence needed yet
            let mut run: Option<(u32, Duration, Instant)> = None; // absent checks, observed time, last
            let mut last_fire: Option<Instant> = None;
            let mut held = false;
            for rec in trace {
                match *rec {
                    Rec::Start => {
                        if !started {
                            (started, paused, armed, run) = (true, false, false, None);
                            free = cfg.fire_if_present_on_start;
                        }
                    }
                    Rec::Restart => {
                        if started {
                            (paused, armed, free, run) = (false, false, false, None);
                        }
                    }
                    Rec::Pause => {
                        if started {
                            (paused, run) = (true, None);
                        }
                    }
                    Rec::Stop => started = false,
                    Rec::Press => {
                        prop_assert!(!held, "invariant 6: a press while the previous one is held");
                        held = true;
                    }
                    Rec::Release => {
                        prop_assert!(held, "invariant 6: a release without a press");
                        held = false;
                    }
                    Rec::StaleRelease => {
                        return Err(TestCaseError::fail(
                            "invariant 6: a click came due while the last one was held",
                        ));
                    }
                    Rec::Check { t, v, step } => {
                        if !started || paused {
                            // Invariant 5 among others: nothing fires unless started and in front.
                            prop_assert_eq!(step, Step::None, "fired while stopped or paused");
                            continue;
                        }
                        if step.is_fire_attempt() {
                            prop_assert_eq!(v, Verdict::Enter, "invariant 1: fired on {:?}", v);
                            // Invariants 2 and 4 (ADR 0007: every (re)start needs an observed absence).
                            prop_assert!(armed || free, "invariant 2/4: fired without an observed absence");
                            if step == Step::Fire {
                                if let Some(prev) = last_fire {
                                    prop_assert!(
                                        t - prev >= cfg.min_fire_interval,
                                        "invariant 3: clicks {:?} apart",
                                        t - prev
                                    );
                                }
                                last_fire = Some(t);
                            }
                            (armed, free, run) = (false, false, None);
                            if step == Step::SafetyStop {
                                started = false;
                            }
                            continue;
                        }
                        // Speed on the rising edge: armed, no confirmation, an Enter fires at once.
                        if (armed || free) && confirm_off {
                            prop_assert!(v != Verdict::Enter, "an armed Enter didn't fire");
                        }
                        if v == Verdict::Absent {
                            let (n, observed) = match run {
                                Some((n, observed, last)) => (n + 1, observed + (t - last).min(cfg.max_check_gap)),
                                None => (1, Duration::ZERO),
                            };
                            run = Some((n, observed, t));
                            if n >= cfg.rearm_absent_samples && observed >= cfg.rearm_absent {
                                armed = true;
                            }
                        } else {
                            run = None;
                        }
                    }
                }
            }
            prop_assert!(!held, "invariant 6: input still held at the end");
            Ok(())
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(2000))]

            #[test]
            fn the_six_invariants_hold(
                (cfg, hold) in config(),
                segments in prop::collection::vec(segment(), 1..60),
            ) {
                let mut events = vec![Ev::Start];
                events.extend(segments.into_iter().flatten());
                let trace = run(cfg, hold, &events);
                check(&cfg, &trace)?;
            }
        }

        #[test]
        fn the_generator_produces_clicks_and_suppressions_and_safety_stops() {
            // Guards the property test against streams in which nothing ever fires.
            let mut runner = proptest::test_runner::TestRunner::deterministic();
            let (mut fires, mut suppressed, mut stops) = (0, 0, 0);
            for _ in 0..300 {
                let (cfg, hold) = config().new_tree(&mut runner).unwrap().current();
                let segments = prop::collection::vec(segment(), 1..60)
                    .new_tree(&mut runner)
                    .unwrap()
                    .current();
                let mut events = vec![Ev::Start];
                events.extend(segments.into_iter().flatten());
                for rec in run(cfg, hold, &events) {
                    match rec {
                        Rec::Check { step: Step::Fire, .. } => fires += 1,
                        Rec::Check {
                            step: Step::Suppressed, ..
                        } => suppressed += 1,
                        Rec::Check {
                            step: Step::SafetyStop, ..
                        } => stops += 1,
                        _ => {}
                    }
                }
            }
            // 1641, 340 and 104 with the deterministic seed.
            assert!(
                fires > 1000 && suppressed > 100 && stops > 30,
                "fires {fires}, suppressed {suppressed}, stops {stops}"
            );
        }
    }
}
