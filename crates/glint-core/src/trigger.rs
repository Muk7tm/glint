// SPDX-License-Identifier: GPL-3.0-or-later
//! The trigger state machine (PLAN §5.5, ADR 0007).
//!
//! **Speed on the rising edge:** once armed, the first Enter check fires, with no waiting
//! (unless you turn on confirmation). **Safety on the falling edge:** firing latches, and only a
//! *watched* absence of at least `rearm_absent` and `rearm_absent_samples` checks re-arms: two
//! checks further apart than the check gap mean Glint was blind in between, and the count starts
//! over. Every (re)start begins latched, so a color that was already showing can't click. The
//! minimum gap and the safety stop sit behind the latch, so they can suppress a click but never
//! add one. When blind time may have hidden a real disappearance, the trigger doesn't click and
//! says so ([`Step::MaybeMissed`]), so a possible miss is never silent.
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
    /// Two checks further apart than this mean Glint was blind in between: a run of absent checks
    /// starts over (ADR 0007). Use [`TriggerCfg::check_gap`].
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
    /// the Max preset, which doesn't pace. Anything on screen for longer than this can't slip
    /// between two watched checks.
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
    /// No click. The color is back after being gone at least `rearm_absent`, so this may be a new
    /// appearance, but Glint was blind for part of that time (a stall) and can't tell whether the
    /// color really stayed away. Not clicking is the only choice that can't double-click
    /// (ADR 0007); the engine reports it so a possible miss is never silent.
    MaybeMissed,
}

impl Step {
    /// The trigger fired on this check, whether or not it clicked.
    pub fn is_fire_attempt(self) -> bool {
        matches!(self, Self::Fire | Self::Suppressed | Self::SafetyStop)
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
    Latched(Latch),
}

/// What the trigger tracks while latched.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Latch {
    /// The previous check.
    prev: Option<Instant>,
    /// When the color was last seen (the fire, or a later Present or Enter check), or when
    /// watching (re)started, before anything was seen.
    since: Option<Instant>,
    /// Glint was blind at least once since `since`.
    blind: bool,
    /// The current watched run of absent checks: when it began, and how many checks.
    run_start: Option<Instant>,
    run_checks: u32,
}

impl Latch {
    /// Watching (re)started at `t`: the time until the first check counts as blind if it's long.
    fn began(t: Instant) -> Self {
        Self {
            prev: Some(t),
            since: Some(t),
            ..Self::default()
        }
    }

    fn seen_at(t: Instant) -> Self {
        Self::began(t)
    }
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

    /// A manual Start at `now`: the Start button, the hotkey, a `start` request or
    /// `--start-on-launch`. Watching begins latched, so the color has to be seen absent before
    /// anything can fire (R7, ADR 0007), unless `fire_if_present_on_start` is on. Does nothing
    /// while already started.
    pub fn start(&mut self, now: Instant) {
        if self.phase == Phase::Stopped {
            self.phase = if self.cfg.fire_if_present_on_start {
                Phase::Idle
            } else {
                Phase::Latched(Latch::began(now))
            };
        }
    }

    /// Watching restarts cleanly at `now`: the target moved or was resized, the monitors changed,
    /// the profile was swapped, or the target window came back to the front. It begins latched with
    /// a fresh absence count whatever the color is doing. Does nothing while stopped, so only a
    /// manual Start ever starts watching, including after a safety stop (ADR 0007).
    pub fn restart(&mut self, now: Instant) {
        if self.phase != Phase::Stopped {
            self.phase = Phase::Latched(Latch::began(now));
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
            Phase::Latched(mut latch) => {
                // Two checks further apart than the check gap: Glint was blind in between, and
                // the color may have come and gone unseen (ADR 0007).
                let blind = latch
                    .prev
                    .is_some_and(|p| now.saturating_duration_since(p) > self.cfg.max_check_gap);
                latch.prev = Some(now);
                latch.blind |= blind;
                let since = *latch.since.get_or_insert(now);
                if v.present() {
                    // The same appearance, unless blind time hid a disappearance long enough to
                    // end it. Then this may be a new appearance: report it, but never click.
                    let maybe_missed = latch.blind && now.saturating_duration_since(since) >= self.cfg.rearm_absent;
                    self.phase = Phase::Latched(Latch::seen_at(now));
                    return if maybe_missed { Step::MaybeMissed } else { Step::None };
                }
                // Only absence watched without a blind gap counts (§5.5): a gap starts the run over.
                if blind {
                    (latch.run_start, latch.run_checks) = (None, 0);
                }
                let run_start = *latch.run_start.get_or_insert(now);
                latch.run_checks = latch.run_checks.saturating_add(1);
                let rearmed = latch.run_checks >= self.cfg.rearm_absent_samples
                    && now.saturating_duration_since(run_start) >= self.cfg.rearm_absent;
                self.phase = if rearmed { Phase::Idle } else { Phase::Latched(latch) };
                Step::None
            }
        }
    }

    fn fire(&mut self, now: Instant) -> Step {
        // 1. Consume the appearance first, so nothing below can lead to a late click.
        self.phase = Phase::Latched(Latch::seen_at(now));
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
        t.start(t0);
        assert_eq!(t.step(t0, Verdict::Enter), Step::Fire);
        t.set_cfg(cfg);
        (t, t0)
    }

    /// Checks with the same verdict every `every` from `from` (exclusive) through `to`
    /// (inclusive), none of which may do anything.
    fn steady(t: &mut Trigger, from: Instant, to: Instant, every: Duration, v: Verdict) {
        let mut now = from;
        while now + every <= to {
            now += every;
            assert_eq!(t.step(now, v), Step::None, "{v:?} at {:?}", now - from);
        }
    }

    fn absent(t: &mut Trigger, from: Instant, to: Instant, every: Duration) {
        steady(t, from, to, every, Verdict::Absent);
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
        t.start(t0);
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
        t.start(t0);
        assert_eq!(t.state(), TriggerState::Latched);
        assert_eq!(t.step(t0, Verdict::Absent), Step::None);
        assert_eq!(t.step(t0 + ms(1), Verdict::Enter), Step::None);
    }

    #[test]
    fn a_restart_during_an_appearance_never_clicks() {
        // The color stays on screen while the window moves; the first capture after the move
        // happens to read a stale frame without it.
        let (mut t, t0) = fired(cfg());
        t.restart(t0 + ms(4));
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
        t.start(t0);
        assert_eq!(t.step(t0, Verdict::Enter), Step::Fire);
        absent(&mut t, t0, t0 + ms(60), ms(1));
        t.restart(t0 + ms(60)); // the window came back to the front with the color showing
        assert_eq!(t.step(t0 + ms(61), Verdict::Enter), Step::None);
        // A second Start while started changes nothing.
        t.start(t0 + ms(61));
        assert_eq!(t.step(t0 + ms(62), Verdict::Enter), Step::None);
    }

    #[test]
    fn a_stall_starts_the_absence_count_over() {
        // ADR 0007: the engine stalls for 50 ms between two absent checks. Those 50 ms weren't
        // watched, so the count starts again after the stall.
        let (mut t, t0) = fired(cfg());
        assert_eq!(t.step(t0 + ms(1), Verdict::Absent), Step::None);
        assert_eq!(t.step(t0 + ms(51), Verdict::Absent), Step::None); // blind for 50 ms: start over
        absent(&mut t, t0 + ms(51), t0 + ms(90), ms(1)); // 39 ms watched
        assert_eq!(t.state(), TriggerState::Latched);
        assert_eq!(t.step(t0 + ms(91), Verdict::Absent), Step::None); // 40 ms
        assert_eq!(t.state(), TriggerState::Idle);
    }

    #[test]
    fn a_stall_between_two_dropouts_cant_fake_a_disappearance() {
        // The color is present except for two short dropouts. A 45 ms stall hides the presence
        // between them, so the trigger sees "absent ... absent" 45 ms apart. Counting the stall
        // as absence (the plan's original sketch) would re-arm and click again. The trigger
        // can't know, so it doesn't click and reports a possible miss.
        let (mut t, t0) = fired(cfg());
        steady(&mut t, t0, t0 + ms(99), ms(1), Verdict::Enter);
        assert_eq!(t.step(t0 + ms(100), Verdict::Absent), Step::None); // first dropout
        assert_eq!(t.step(t0 + ms(145), Verdict::Absent), Step::None); // second dropout, after the stall
        assert_eq!(t.step(t0 + ms(146), Verdict::Enter), Step::MaybeMissed);
        assert_eq!(t.state(), TriggerState::Latched);
    }

    #[test]
    fn absence_watched_before_a_stall_doesnt_count_either() {
        // Gone for 31 ms, blind for 45 ms (the color came back and left again unseen), then gone
        // for 7 ms more. This rule's first version credited a stall with 2 ms and kept what was
        // watched before it: 31 + 2 + 7 = 40 ms, which re-armed and clicked a second time within
        // one appearance. Only a run with no blind gap counts now.
        let (mut t, t0) = fired(cfg());
        steady(&mut t, t0, t0 + ms(10), ms(1), Verdict::Enter);
        absent(&mut t, t0 + ms(10), t0 + ms(42), ms(1)); // 11..=42: 31 ms gone
        assert_eq!(t.step(t0 + ms(87), Verdict::Absent), Step::None); // after 45 ms blind
        absent(&mut t, t0 + ms(87), t0 + ms(94), ms(1)); // 7 ms more
        assert_eq!(t.step(t0 + ms(95), Verdict::Enter), Step::MaybeMissed);
        assert_eq!(t.state(), TriggerState::Latched);
    }

    #[test]
    fn a_blind_gap_long_enough_to_hide_a_whole_disappearance_is_reported() {
        let (mut t, t0) = fired(cfg());
        assert_eq!(t.step(t0 + ms(1), Verdict::Enter), Step::None);
        // Blind for 20 ms: too short to hide a 40 ms absence, so nothing can have been missed.
        assert_eq!(t.step(t0 + ms(21), Verdict::Enter), Step::None);
        // Blind for 49 ms: the color could have left for 40 ms and come back unseen.
        assert_eq!(t.step(t0 + ms(70), Verdict::Enter), Step::MaybeMissed);
        assert_eq!(t.step(t0 + ms(71), Verdict::Enter), Step::None);
    }

    #[test]
    fn the_minimum_gap_suppresses_and_consumes_the_appearance() {
        let c = TriggerCfg {
            rearm_absent: ms(0),
            rearm_absent_samples: 1,
            ..cfg()
        };
        let (mut t, t0) = fired(c);
        assert_eq!(t.step(t0 + ms(1), Verdict::Absent), Step::None); // re-armed at once
        assert_eq!(t.step(t0 + ms(2), Verdict::Enter), Step::Suppressed); // 2 ms < 25 ms
        assert_eq!(t.step(t0 + ms(3), Verdict::Enter), Step::None); // still the same appearance
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
        t.restart(t0 + ms(1999));
        t.pause();
        assert_eq!(t.step(t0 + ms(2000), Verdict::Enter), Step::None);
        t.start(t0 + ms(2000));
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
        t.start(t0);
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
        t.start(t0);
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
        t.start(t0);
        t.pause();
        assert_eq!(t.state(), TriggerState::Paused);
        assert!(!t.is_watching());
        assert_eq!(t.step(t0, Verdict::Enter), Step::None);
        t.restart(t0); // back in front
        assert_eq!(t.state(), TriggerState::Latched);
        assert_eq!(t.step(t0 + ms(1), Verdict::Enter), Step::None);
        t.stop();
        t.restart(t0 + ms(2));
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
            Start(Instant),
            Restart(Instant),
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
                            Step::None | Step::Suppressed | Step::MaybeMissed => {}
                        }
                    }
                    Ev::Start => {
                        trigger.start(now);
                        trace.push(Rec::Start(now));
                    }
                    Ev::Restart => {
                        release(&mut held, &mut trace);
                        trigger.restart(now);
                        trace.push(Rec::Restart(now));
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

        /// Checks the trace against the invariants, with its own model of watched absence.
        fn check(cfg: &TriggerCfg, trace: &[Rec]) -> Result<(), TestCaseError> {
            let confirm_off = cfg.confirm_samples <= 1 && cfg.confirm_min.is_zero();
            let mut started = false; // between a manual Start and a stop
            let mut paused = false;
            let mut armed = false; // a watched absence since the last (re)start or fire
            let mut free = false; // a manual Start with fire_if_present_on_start: no absence needed yet
            // While latched: the previous check, when the color was last seen, whether Glint was
            // blind since then, and the watched absence run (its first check and length).
            let mut prev: Option<Instant> = None;
            let mut seen: Option<Instant> = None;
            let mut blind = false;
            let mut run: Option<(Instant, u32)> = None;
            let mut last_fire: Option<Instant> = None;
            let mut held = false;
            for rec in trace {
                match *rec {
                    Rec::Start(t) => {
                        if !started {
                            (started, paused, armed, free) = (true, false, false, cfg.fire_if_present_on_start);
                            (prev, seen, blind, run) = (Some(t), Some(t), false, None);
                        }
                    }
                    Rec::Restart(t) => {
                        if started {
                            (paused, armed, free) = (false, false, false);
                            (prev, seen, blind, run) = (Some(t), Some(t), false, None);
                        }
                    }
                    Rec::Pause => {
                        if started {
                            paused = true;
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
                            prop_assert_eq!(step, Step::None, "acted while stopped or paused");
                            continue;
                        }
                        if step.is_fire_attempt() {
                            prop_assert_eq!(v, Verdict::Enter, "invariant 1: fired on {:?}", v);
                            // Invariants 2 and 4 (ADR 0007: every (re)start needs a watched absence).
                            prop_assert!(armed || free, "invariant 2/4: fired without a watched absence");
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
                            (armed, free) = (false, false);
                            (prev, seen, blind, run) = (Some(t), Some(t), false, None);
                            if step == Step::SafetyStop {
                                started = false;
                            }
                            continue;
                        }
                        if armed || free {
                            // Speed on the rising edge: armed, no confirmation, an Enter fires at once.
                            if confirm_off {
                                prop_assert!(v != Verdict::Enter, "an armed Enter didn't fire");
                            }
                            prop_assert_eq!(step, Step::None, "reported a possible miss while armed");
                            continue;
                        }
                        // Latched: two checks further apart than the check gap mean Glint was blind.
                        let gap_blind = prev.is_some_and(|p| t - p > cfg.max_check_gap);
                        prev = Some(t);
                        blind |= gap_blind;
                        let since = *seen.get_or_insert(t);
                        if v == Verdict::Absent {
                            prop_assert_eq!(step, Step::None);
                            if gap_blind {
                                run = None;
                            }
                            let (start, n) = run.map_or((t, 1), |(start, n)| (start, n + 1));
                            run = Some((start, n));
                            if n >= cfg.rearm_absent_samples && t - start >= cfg.rearm_absent {
                                armed = true;
                            }
                        } else {
                            // A possible miss is reported exactly when blind time may have hidden
                            // a long enough disappearance.
                            let expected = blind && t - since >= cfg.rearm_absent;
                            prop_assert_eq!(step == Step::MaybeMissed, expected, "possible miss at {:?}", t);
                            (seen, blind, run) = (Some(t), false, None);
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
        fn the_generator_produces_every_kind_of_step() {
            // Guards the property test against streams in which nothing ever fires.
            let mut runner = proptest::test_runner::TestRunner::deterministic();
            let (mut fires, mut suppressed, mut stops, mut maybe) = (0, 0, 0, 0);
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
                        Rec::Check {
                            step: Step::MaybeMissed,
                            ..
                        } => maybe += 1,
                        _ => {}
                    }
                }
            }
            // 1215, 305, 72 and 7008 with the deterministic seed.
            assert!(
                fires > 800 && suppressed > 100 && stops > 30 && maybe > 1000,
                "fires {fires}, suppressed {suppressed}, stops {stops}, possible misses {maybe}"
            );
        }
    }

    mod ground_truth {
        //! The property that matters most (R1): never two clicks in one *real* appearance. A real
        //! on/off color is simulated in continuous time and sampled about 2,000 times a second,
        //! with jitter and random stalls of up to 80 ms. Appearances come from the true signal
        //! (gaps shorter than `rearm_absent` join), not from what the trigger saw. Every missed
        //! appearance must have a reason, and a miss caused by blind time must be reported.

        use proptest::prelude::*;

        use super::*;

        /// Every on-screen state lasts at least this long, longer than the 2 ms check gap: screen
        /// content changes at most once per frame, and a frame lasts 4.2 ms even at 240 Hz.
        const MIN_STATE_US: u64 = 2_500;

        fn lcg(state: &mut u64) -> u64 {
            *state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            *state >> 33
        }

        /// A true appearance: first on, last off, and how long the color was off before it (µs).
        type Appearance = (u64, u64, u64);

        struct Outcome {
            appearances: Vec<Appearance>,
            fires: Vec<u64>,
            maybe_missed: Vec<u64>,
            /// Checks that saw the color.
            seen: Vec<u64>,
            safety_stop: Option<u64>,
        }

        fn simulate(rearm_ms: u64, samples: u32, segments: &[(u64, u64)], seed: u64, stall_permille: u64) -> Outcome {
            // The true signal: off, then on, again and again (µs from Start).
            let mut on = Vec::new();
            let mut t = 0;
            for &(off, dur) in segments {
                t += off;
                on.push((t, t + dur));
                t += dur;
            }
            let cfg = TriggerCfg {
                confirm_samples: 1,
                confirm_min: Duration::ZERO,
                rearm_absent: Duration::from_millis(rearm_ms),
                rearm_absent_samples: samples,
                min_fire_interval: TriggerCfg::min_gap(Duration::ZERO, Duration::ZERO),
                max_clicks_per_sec: MAX_CLICKS_PER_SEC_LIMIT,
                fire_if_present_on_start: false,
                max_check_gap: TriggerCfg::check_gap(Some(Duration::from_micros(500))),
            };
            let t0 = Instant::now();
            let mut trigger = Trigger::new(cfg);
            trigger.start(t0);
            let mut out = Outcome {
                appearances: Vec::new(),
                fires: Vec::new(),
                maybe_missed: Vec::new(),
                seen: Vec::new(),
                safety_stop: None,
            };
            let (end, mut now, mut seed, mut i) = (t + 100_000, 0u64, seed, 0usize);
            while now < end {
                let r = lcg(&mut seed);
                now += if r % 1000 < stall_permille {
                    2_001 + r % 78_000
                } else {
                    400 + r % 300
                };
                while i < on.len() && on[i].1 <= now {
                    i += 1;
                }
                let present = i < on.len() && on[i].0 <= now;
                if present {
                    out.seen.push(now);
                }
                let v = if present { Verdict::Enter } else { Verdict::Absent };
                match trigger.step(t0 + Duration::from_micros(now), v) {
                    Step::Fire => out.fires.push(now),
                    Step::MaybeMissed => out.maybe_missed.push(now),
                    Step::SafetyStop => {
                        out.safety_stop = Some(now);
                        break;
                    }
                    Step::None | Step::Suppressed => {}
                }
            }
            let mut last_end = 0;
            for &(s, e) in &on {
                match out.appearances.last_mut() {
                    Some(a) if s - a.1 < rearm_ms * 1000 => a.1 = e,
                    _ => out.appearances.push((s, e, s - last_end)),
                }
                last_end = e;
            }
            out
        }

        fn within(times: &[u64], (start, end, _): Appearance) -> usize {
            times.iter().filter(|&&t| start <= t && t < end).count()
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(1000))]

            #[test]
            fn never_two_clicks_in_one_real_appearance(
                rearm_ms in 0u64..=60,
                samples in 1u32..=3,
                segments in prop::collection::vec((MIN_STATE_US..120_000, MIN_STATE_US..120_000), 1..25),
                seed in any::<u64>(),
                stall_permille in prop_oneof![Just(0u64), 1u64..=40],
            ) {
                let out = simulate(rearm_ms, samples, &segments, seed, stall_permille);
                for &a in &out.appearances {
                    let clicks = within(&out.fires, a);
                    prop_assert!(clicks <= 1, "{} clicks in the appearance {:?}", clicks, a);
                    if clicks == 0 {
                        let stopped = out.safety_stop.is_some_and(|t| t < a.1);
                        let unseen = within(&out.seen, a) == 0; // it was over before Glint could look
                        let reported = within(&out.maybe_missed, a) > 0;
                        // Gone for barely longer than "Gone after": the 2 ms check gap at both ends.
                        let boundary = a.2 < rearm_ms * 1000 + 3_000;
                        prop_assert!(stopped || unseen || reported || boundary, "silently missed {:?}", a);
                    }
                }
                prop_assert!(out.fires.iter().all(|&f| out.appearances.iter().any(|&(s, e, _)| s <= f && f < e)));
                // Never blind, never unsure: with no stalls there is nothing to report.
                if stall_permille == 0 {
                    prop_assert!(out.maybe_missed.is_empty());
                }
            }
        }
    }
}
