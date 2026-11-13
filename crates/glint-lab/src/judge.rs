// SPDX-License-Identifier: GPL-3.0-or-later
//! Judging a run (§16). The lab's drawing is replayed against the trigger's rules (§5.5,
//! ADR 0007) to find each appearance that must, or may, get a click, and every click the engine
//! sent is matched to one.
//!
//! A client can't see the moment the server carried out a drawing request, only that it came
//! after the request went out and before its round trip returned ([`Drawn`]). So every
//! on-screen duration has a shortest and a longest possible value, and when an absence could
//! fall on either side of *Gone after* (give or take the check gap at each end), the next
//! appearance's click is optional. Everything else is strict:
//!
//! - a click more than the rules allow always fails the run;
//! - a missing click fails it, unless the engine reported a possible miss (ADR 0007), or was
//!   blind for the whole time the color was surely showing, or the color was never surely on
//!   screen long enough to be seen (an X server stall can squeeze an appearance out).
//!
//! Clicks are the canvas's button presses: every judged run clicks with the mouse.

use std::time::{Duration, Instant};

use glint_core::{Class, TriggerCfg};
use glint_ipc::{StopCause, Trace, instant_at};

use crate::canvas::{Drawn, Input, InputKind};
use crate::probe::ProbeResult;

/// A click slower than this is flagged as late (the 10 ms ceiling of §3), never failed.
pub const LATE: Duration = Duration::from_millis(10);
/// The v1.0 timing targets (ADR 0004), judged only on the real desktop.
pub const TARGET_P99: Duration = Duration::from_millis(2);
pub const TARGET_MAX: Duration = Duration::from_millis(10);
/// The shortest time a color must surely be on screen to count as shown: the ground-truth
/// test's shortest state, longer than the 2 ms check gap (ADR 0007).
const MIN_VISIBLE: Duration = Duration::from_micros(2_500);
/// How long after the run's end a click still counts toward the last appearance.
const TAIL: Duration = Duration::from_millis(100);

/// What the watched spot shows, by the rules in force.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shown {
    Absent,
    /// In the wide range only: enough to stay, not to start (§5.4).
    Wide,
    Tight,
}

impl From<Class> for Shown {
    fn from(c: Class) -> Self {
        match c {
            Class::Tight => Self::Tight,
            Class::Wide => Self::Wide,
            Class::Absent | Class::Excluded => Self::Absent,
        }
    }
}

/// What happened to the engine at a mark.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change {
    /// Nothing: the lab drew.
    Draw,
    /// Watching (re)started: a Start, or a profile change while started. The trigger latches,
    /// with a fresh absence count (ADR 0007).
    Restart,
    /// Watching stopped: a Stop, or a self-stop from the trace.
    Stop,
}

/// From `at` on, the watched spot showed `shown`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Mark {
    pub at: Drawn,
    pub shown: Shown,
    pub change: Change,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct JudgeCfg {
    /// *Gone after*.
    pub gone_after: Duration,
    /// How far the engine's measure of an absence can fall short of the truth: it counts only
    /// what was surely watched, which can miss a check gap at each end (ADR 0011).
    pub slack: Duration,
    /// Judge the v1.0 timing targets too (real desktop, timing scenarios).
    pub timing: bool,
    /// The self-stop the scenario provokes on purpose.
    pub expect_stop: Option<StopCause>,
}

impl JudgeCfg {
    pub fn new(gone_after: Duration, period: Option<Duration>) -> Self {
        Self {
            gone_after,
            slack: TriggerCfg::check_gap(period) * 2,
            timing: false,
            expect_stop: None,
        }
    }
}

/// Everything a run recorded.
pub struct Evidence<'a> {
    /// In the order they happened.
    pub marks: &'a [Mark],
    /// When the run ended; the last mark lasts until then.
    pub end: Instant,
    pub inputs: &'a [Input],
    pub trace: &'a [Trace],
    pub probe: Option<&'a ProbeResult>,
}

/// The median time of each stage of a response (§16): waiting for the next check, the
/// capture's round trip, the decision, writing the press, and its way to the canvas.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Split {
    pub wait: Duration,
    pub capture: Duration,
    pub decide: Duration,
    pub inject: Duration,
    pub deliver: Duration,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Report {
    pub title: String,
    /// Appearances that call for a click by the rules.
    pub appearances: u32,
    /// Of those, the ones whose click was optional (an absence right at *Gone after*).
    pub optional: u32,
    /// Presses the engine sent.
    pub clicks: u32,
    pub extra: u32,
    /// Required clicks that never came, with nothing to explain them.
    pub missed: u32,
    /// Missing clicks the engine reported as possible misses.
    pub reported: u32,
    /// Missing clicks for appearances the engine had no chance to see.
    pub unseen: u32,
    /// Missing clicks once the engine had stopped itself.
    pub after_stop: u32,
    /// Responses of the clicked appearances that no X server stall touched.
    pub latencies: Vec<Duration>,
    /// Responses of the clicked appearances an X server stall touched, with the stall's length.
    pub stalled: Vec<(Duration, Duration)>,
    /// Responses slower than [`LATE`], from either list.
    pub late: Vec<Duration>,
    /// The X server stalls the probe saw, by length.
    pub server_stalls: Vec<Duration>,
    pub probed: bool,
    pub split: Option<Split>,
    /// The probe's p99 round trip before and while watching.
    pub server_tax: Option<(Duration, Duration)>,
    /// When and why the engine stopped itself.
    pub self_stops: Vec<(Instant, StopCause)>,
    /// When the engine wrote each press, from its trace.
    pub fired_at: Vec<Instant>,
    pub expect_stop: Option<StopCause>,
    /// The v1.0 timing targets are part of the verdict.
    pub timing: bool,
    /// The run was in the sandbox, where timing means little.
    pub sandbox: bool,
    pub problems: Vec<String>,
    pub notes: Vec<String>,
    /// What the engine traced around each unexplained miss.
    pub missed_detail: Vec<String>,
}

fn quantile(sorted: &[Duration], q: f64) -> Option<Duration> {
    let i = ((sorted.len() as f64 * q) as usize).min(sorted.len().checked_sub(1)?);
    sorted.get(i).copied()
}

fn ms(d: Duration) -> String {
    format!("{:.2}", d.as_secs_f64() * 1e3)
}

impl Report {
    pub fn named(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }

    fn unexpected_stops(&self) -> usize {
        self.self_stops
            .iter()
            .filter(|(_, c)| Some(*c) != self.expect_stop)
            .count()
    }

    fn sorted_latencies(&self) -> Vec<Duration> {
        let mut l = self.latencies.clone();
        l.sort_unstable();
        l
    }

    /// p99 ≤ 2 ms and max ≤ 10 ms over the trials no X server stall touched (ADR 0004).
    pub fn timing_ok(&self) -> bool {
        let l = self.sorted_latencies();
        quantile(&l, 0.99).is_none_or(|p| p <= TARGET_P99) && l.last().is_none_or(|&m| m <= TARGET_MAX)
    }

    pub fn passed(&self) -> bool {
        self.extra == 0
            && self.missed == 0
            && self.unexpected_stops() == 0
            && self.problems.is_empty()
            && (!self.timing || self.timing_ok())
    }

    /// The §16 report.
    pub fn summary(&self) -> String {
        let mut out = format!("glint-lab {}\n", self.title);
        let mut line = |label: &str, text: String| out.push_str(&format!("{label:<11} {text}\n"));
        line(
            "appearances",
            format!(
                "{} · clicks {} · extra {} · missed {} · possible misses reported {}",
                self.appearances, self.clicks, self.extra, self.missed, self.reported
            ),
        );
        if self.unseen > 0 {
            line(
                "unseen",
                format!(
                    "{} appearances came and went while Glint was blind, or were never surely on screen (listed separately)",
                    self.unseen
                ),
            );
        }
        if self.optional > 0 {
            line(
                "optional",
                format!(
                    "{} appearances came back right at Gone after, so a click was allowed either way",
                    self.optional
                ),
            );
        }
        if self.probed {
            let worst = self
                .server_stalls
                .iter()
                .max()
                .map(|&w| format!(" (worst {} ms)", ms(w)));
            line(
                "stalls",
                format!(
                    "X server stalls during the run {}{} · trials that coincided {} (listed separately)",
                    self.server_stalls.len(),
                    worst.unwrap_or_default(),
                    self.stalled.len()
                ),
            );
            for (response, stall) in self.stalled.iter().take(5) {
                line(
                    "",
                    format!("response {} ms during a {} ms stall", ms(*response), ms(*stall)),
                );
            }
        }
        let l = self.sorted_latencies();
        if let (Some(p50), Some(p95), Some(p99), Some(max)) =
            (quantile(&l, 0.5), quantile(&l, 0.95), quantile(&l, 0.99), l.last())
        {
            line(
                "response",
                format!(
                    "p50 {} ms · p95 {} ms · p99 {} ms · max {} ms",
                    ms(p50),
                    ms(p95),
                    ms(p99),
                    ms(*max)
                ),
            );
        }
        if let Some(s) = self.split {
            let f = |d: Duration| format!("{:.3}", d.as_secs_f64() * 1e3);
            line(
                "split",
                format!(
                    "wait {} · capture {} · decide {} · inject {} · deliver {} ms",
                    f(s.wait),
                    f(s.capture),
                    f(s.decide),
                    f(s.inject),
                    f(s.deliver)
                ),
            );
        }
        if let Some((before, during)) = self.server_tax {
            let delta = during.as_secs_f64() - before.as_secs_f64();
            line(
                "server tax",
                format!(
                    "other clients' round trips {:+.2} ms at p99 ({} → {} ms)",
                    delta * 1e3,
                    ms(before),
                    ms(during)
                ),
            );
        }
        if !self.late.is_empty() {
            let list: Vec<String> = self.late.iter().take(8).map(|&d| ms(d)).collect();
            line(
                "late",
                format!(
                    "{} clicks over 10 ms (flagged, not failed): {} ms",
                    self.late.len(),
                    list.join(", ")
                ),
            );
        }
        for (_, cause) in &self.self_stops {
            let expected = if Some(*cause) == self.expect_stop {
                "expected"
            } else {
                "unexpected"
            };
            line("stopped", format!("the engine stopped itself: {cause:?} ({expected})"));
        }
        if self.after_stop > 0 {
            line(
                "after stop",
                format!("{} appearances after the engine stopped itself", self.after_stop),
            );
        }
        for m in &self.missed_detail {
            line("missed", m.clone());
        }
        for p in &self.problems {
            line("problem", p.clone());
        }
        for n in &self.notes {
            line("note", n.clone());
        }
        let conditions = if self.timing {
            "p99 ≤ 2 ms, max ≤ 10 ms, 0 extra, 0 missed"
        } else if self.sandbox {
            "0 extra, 0 missed; timing isn't judged under Xvfb"
        } else {
            "0 extra, 0 missed; timing isn't judged in this scenario"
        };
        let verdict = if self.passed() { "PASS" } else { "FAIL" };
        line("verdict", format!("{verdict}  ({conditions})"));
        out
    }
}

/// Whether the engine could fire when an appearance began, and whether it had to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Expect {
    Must,
    /// The absence before it was right at *Gone after*.
    May,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Episode {
    /// When the appearance's drawing request went out: responses are timed from here.
    start: Instant,
    /// When the color was surely on screen: from its round trip to the next change's request.
    visible: (Instant, Instant),
    expect: Expect,
    /// The absence right before it: shortest and longest.
    gap: (Duration, Duration),
}

/// The marks with the engine's self-stops added as stops.
fn with_stops(marks: &[Mark], stops: &[(Instant, StopCause)]) -> Vec<Mark> {
    let mut out = marks.to_vec();
    for &(t, _) in stops {
        let i = out.partition_point(|m| m.at.sent <= t);
        let shown = i.checked_sub(1).map_or(Shown::Absent, |j| out[j].shown);
        out.insert(
            i,
            Mark {
                at: Drawn { sent: t, drawn: t },
                shown,
                change: Change::Stop,
            },
        );
    }
    out
}

/// Replays the marks through the trigger's rules. The state is a pair of possibilities: the
/// engine may be armed, may be latched, or either when an absence was too close to call.
fn episodes(marks: &[Mark], end: Instant, cfg: &JudgeCfg) -> Vec<Episode> {
    let (mut armed, mut latched) = (false, false);
    // The latched case's absence so far: shortest and longest.
    let mut run: Option<(Duration, Duration)> = None;
    // The absence since the color was last present, whatever the state, for the report.
    let mut gap = (Duration::ZERO, Duration::ZERO);
    let mut out = Vec::new();
    for (i, m) in marks.iter().enumerate() {
        let (next_sent, next_drawn) = marks.get(i + 1).map_or((end, end), |n| (n.at.sent, n.at.drawn));
        let shortest = next_sent.saturating_duration_since(m.at.drawn);
        let longest = next_drawn.saturating_duration_since(m.at.sent);
        match m.change {
            Change::Draw => {}
            Change::Restart => (armed, latched, run) = (false, true, None),
            Change::Stop => (armed, latched, run) = (false, false, None),
        }
        if m.shown == Shown::Absent {
            gap = (gap.0 + shortest, gap.1 + longest);
        }
        match m.shown {
            Shown::Absent if latched => {
                let (lo, hi) = run.unwrap_or_default();
                let (lo, hi) = (lo + shortest, hi + longest);
                if lo >= cfg.gone_after + cfg.slack {
                    (armed, latched, run) = (true, false, None);
                } else {
                    armed |= hi >= cfg.gone_after;
                    run = Some((lo, hi));
                }
            }
            Shown::Absent => {}
            // Present enough to stay: an absence starts over.
            Shown::Wide => {
                if latched {
                    run = None;
                }
                gap = (Duration::ZERO, Duration::ZERO);
            }
            Shown::Tight => {
                if armed {
                    out.push(Episode {
                        start: m.at.sent,
                        visible: (m.at.drawn, next_sent),
                        expect: if latched { Expect::May } else { Expect::Must },
                        gap,
                    });
                }
                if armed || latched {
                    (armed, latched, run) = (false, true, None);
                }
                gap = (Duration::ZERO, Duration::ZERO);
            }
        }
    }
    out
}

/// `t` relative to `t0`, in ms.
fn at(t: Instant, t0: Instant) -> String {
    match t.checked_duration_since(t0) {
        Some(d) => format!("+{:.1}", d.as_secs_f64() * 1e3),
        None => format!("-{:.1}", (t0 - t).as_secs_f64() * 1e3),
    }
}

/// An unexplained miss and what the engine traced around it.
fn describe(
    ep: &Episode,
    window_end: Instant,
    t0: Instant,
    blind: &[(Instant, Instant)],
    maybe: &[Instant],
    fired: &[[Instant; 4]],
) -> String {
    let from = ep.start.checked_sub(Duration::from_millis(150)).unwrap_or(ep.start);
    let mut seen = Vec::new();
    for &(f, t) in blind.iter().filter(|&&(f, t)| t >= from && f <= window_end) {
        seen.push(format!("blind {}–{}", at(f, t0), at(t, t0)));
    }
    for &m in maybe.iter().filter(|&&m| m >= from && m <= window_end) {
        seen.push(format!("possible miss {}", at(m, t0)));
    }
    for f in fired.iter().filter(|f| f[3] >= from && f[3] <= window_end) {
        seen.push(format!("click {}", at(f[3], t0)));
    }
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    format!(
        "appearance at {} ms (on screen by {}, until {}), after a gap of {:.1}–{:.1} ms; engine nearby: {}",
        at(ep.start, t0),
        at(ep.visible.0, t0),
        at(ep.visible.1, t0),
        ms(ep.gap.0),
        ms(ep.gap.1),
        if seen.is_empty() {
            "nothing".into()
        } else {
            seen.join(", ")
        }
    )
}

/// Judges one run.
pub fn judge(title: String, cfg: &JudgeCfg, ev: &Evidence) -> Report {
    let mut r = Report {
        title,
        probed: ev.probe.is_some(),
        expect_stop: cfg.expect_stop,
        timing: cfg.timing,
        ..Report::default()
    };
    let mut fired = Vec::new();
    let mut maybe = Vec::new();
    let mut blind = Vec::new();
    for t in ev.trace {
        match *t {
            Trace::Fired {
                request_ns,
                capture_ns,
                decided_ns,
                sent_ns,
            } => fired.push([request_ns, capture_ns, decided_ns, sent_ns].map(instant_at)),
            Trace::MaybeMissed { at_ns } => maybe.push(instant_at(at_ns)),
            Trace::Blind { from_ns, to_ns } => blind.push((instant_at(from_ns), instant_at(to_ns))),
            Trace::Stopped { at_ns, cause } => r.self_stops.push((instant_at(at_ns), cause)),
        }
    }
    r.fired_at = fired.iter().map(|f| f[3]).collect();
    let marks = with_stops(ev.marks, &r.self_stops);
    let episodes = episodes(&marks, ev.end, cfg);
    let t0 = marks.first().map_or(ev.end, |m| m.at.sent);
    let stalls = ev.probe.map_or(&[][..], |p| &p.stalls[..]);
    r.server_stalls = stalls.iter().map(|&(s, e)| e - s).collect();
    if let Some(p) = ev.probe
        && p.baseline.len() >= 200
        && p.loaded.len() >= 200
    {
        r.server_tax = p.baseline.quantile(0.99).zip(p.loaded.quantile(0.99));
    }

    let presses: Vec<Instant> = ev
        .inputs
        .iter()
        .filter(|i| matches!(i.kind, InputKind::ButtonPress(_)))
        .map(|i| i.at)
        .collect();
    r.clicks = presses.len() as u32;
    let mut per: Vec<Vec<Instant>> = vec![Vec::new(); episodes.len()];
    for &p in &presses {
        match episodes.partition_point(|e| e.start <= p).checked_sub(1) {
            Some(k) => per[k].push(p),
            // Before any appearance that could click.
            None => r.extra += 1,
        }
    }

    let mut stages: [Vec<Duration>; 5] = Default::default();
    for (k, ep) in episodes.iter().enumerate() {
        let window_end = episodes.get(k + 1).map_or(ev.end + TAIL, |e| e.start);
        let clicks = &per[k];
        r.appearances += 1;
        if ep.expect == Expect::May {
            r.optional += 1;
        }
        r.extra += clicks.len().saturating_sub(1) as u32;
        let Some(&first) = clicks.first() else {
            if ep.expect == Expect::Must {
                let (vs, ve) = ep.visible;
                // A self-stop first: it also cuts the visible window short.
                if r.self_stops.iter().any(|&(t, _)| t >= ep.start && t < window_end) {
                    r.after_stop += 1;
                } else if maybe.iter().any(|&t| t >= ep.start && t < window_end) {
                    r.reported += 1;
                } else if ve.saturating_duration_since(vs) < MIN_VISIBLE
                    || blind.iter().any(|&(f, t)| f <= vs && ve <= t)
                {
                    r.unseen += 1;
                } else {
                    r.missed += 1;
                    r.missed_detail
                        .push(describe(ep, window_end, t0, &blind, &maybe, &fired));
                }
            }
            continue;
        };
        let response = first - ep.start;
        if response > LATE {
            r.late.push(response);
        }
        let stall = stalls
            .iter()
            .filter(|&&(s, e)| s < first && e > ep.start)
            .map(|&(s, e)| e - s)
            .max();
        if let Some(len) = stall {
            r.stalled.push((response, len));
            continue;
        }
        r.latencies.push(response);
        // The engine's own record of this click: the last press written before it arrived.
        if let Some(&[request, capture, decided, sent]) = fired.iter().rev().find(|f| f[3] <= first && f[3] >= ep.start)
        {
            let parts = [
                request.saturating_duration_since(ep.start),
                capture.saturating_duration_since(request),
                decided.saturating_duration_since(capture),
                sent.saturating_duration_since(decided),
                first.saturating_duration_since(sent),
            ];
            for (stage, d) in stages.iter_mut().zip(parts) {
                stage.push(d);
            }
        }
    }
    let median = |v: &mut Vec<Duration>| {
        v.sort_unstable();
        quantile(v, 0.5)
    };
    let [w, c, d, i, del] = &mut stages;
    if let (Some(wait), Some(capture), Some(decide), Some(inject), Some(deliver)) =
        (median(w), median(c), median(d), median(i), median(del))
    {
        r.split = Some(Split {
            wait,
            capture,
            decide,
            inject,
            deliver,
        });
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: Duration = Duration::from_millis(40);

    /// Builds marks from (ms, shown, change), with each change on screen 50 µs after its request.
    fn marks(t0: Instant, spec: &[(u64, Shown, Change)]) -> Vec<Mark> {
        spec.iter()
            .map(|&(t, shown, change)| {
                let sent = t0 + Duration::from_millis(t);
                Mark {
                    at: Drawn {
                        sent,
                        drawn: sent + Duration::from_micros(50),
                    },
                    shown,
                    change,
                }
            })
            .collect()
    }

    fn press(t0: Instant, ms: u64) -> Input {
        Input {
            at: t0 + Duration::from_micros(ms * 1000 + 300),
            server_ms: 0,
            kind: InputKind::ButtonPress(1),
        }
    }

    fn run(spec: &[(u64, Shown, Change)], presses: &[u64], trace: &[Trace]) -> Report {
        let t0 = Instant::now();
        let m = marks(t0, spec);
        let inputs: Vec<Input> = presses.iter().map(|&p| press(t0, p)).collect();
        let end = t0 + Duration::from_millis(spec.last().map_or(0, |s| s.0) + 200);
        judge(
            "test".into(),
            &JudgeCfg::new(G, Some(Duration::from_micros(500))),
            &Evidence {
                marks: &m,
                end,
                inputs: &inputs,
                trace,
                probe: None,
            },
        )
    }

    use Change::{Draw, Restart, Stop};
    use Shown::{Absent, Tight, Wide};

    #[test]
    fn one_click_per_appearance() {
        // Start with the screen clear; two appearances with a long gap.
        let spec = [
            (0, Absent, Restart),
            (100, Tight, Draw),
            (300, Absent, Draw),
            (500, Tight, Draw),
            (600, Absent, Draw),
        ];
        let r = run(&spec, &[100, 500], &[]);
        assert_eq!((r.appearances, r.clicks, r.extra, r.missed), (2, 2, 0, 0));
        assert!(r.passed(), "{}", r.summary());
        assert_eq!(r.latencies, [Duration::from_micros(300); 2]);
        // A second click in one appearance, and a missing one.
        let r = run(&spec, &[100, 150], &[]);
        assert_eq!((r.extra, r.missed), (1, 1));
        assert!(!r.passed());
        assert!(r.summary().contains("extra 1 · missed 1"), "{}", r.summary());
    }

    #[test]
    fn nothing_fires_right_after_a_start_or_while_stopped() {
        // Start while the color shows (arm-while-present): no click until it leaves and returns.
        let spec = [
            (0, Tight, Restart),
            (200, Absent, Draw),
            (300, Tight, Draw),
            (400, Tight, Stop),
            (450, Absent, Draw),
            (550, Tight, Draw),
        ];
        let r = run(&spec, &[300], &[]);
        assert_eq!((r.appearances, r.extra, r.missed), (1, 0, 0), "{}", r.summary());
        // A click while it was showing at the start is extra.
        assert_eq!(run(&spec, &[10, 300], &[]).extra, 1);
        // So is one while stopped.
        assert_eq!(run(&spec, &[300, 560], &[]).extra, 1);
        // An appearance less than Gone after past the start can't click either.
        let r = run(&[(0, Absent, Restart), (30, Tight, Draw)], &[], &[]);
        assert_eq!(r.appearances, 0);
    }

    #[test]
    fn gaps_shorter_than_gone_after_merge() {
        // 25 ms gaps merge (rapid, flicker); 55 ms gaps don't; 41 ms is too close to call.
        let spec = [
            (0, Absent, Restart),
            (100, Tight, Draw),
            (150, Absent, Draw),
            (175, Tight, Draw), // merged
            (225, Absent, Draw),
            (280, Tight, Draw), // after 55 ms: a new appearance
            (330, Absent, Draw),
            (371, Tight, Draw), // after 41 ms: optional
            (420, Absent, Draw),
        ];
        let r = run(&spec, &[100, 280, 371], &[]);
        assert_eq!(
            (r.appearances, r.optional, r.extra, r.missed),
            (3, 1, 0, 0),
            "{}",
            r.summary()
        );
        assert!(run(&spec, &[100, 280], &[]).passed());
        // A click for the merged appearance is extra.
        assert_eq!(run(&spec, &[100, 175, 280], &[]).extra, 1);
        // Wide-only in between keeps the color present: no new appearance.
        let noisy = [
            (0, Absent, Restart),
            (100, Tight, Draw),
            (110, Wide, Draw),
            (190, Tight, Draw),
            (200, Wide, Draw),
            (260, Absent, Draw),
        ];
        assert_eq!(run(&noisy, &[100], &[]).appearances, 1);
    }

    #[test]
    fn excused_misses() {
        let t0 = Instant::now();
        let spec = [
            (0, Absent, Restart),
            (100, Tight, Draw),
            (200, Absent, Draw),
            (400, Tight, Draw),
            (500, Absent, Draw),
        ];
        let ns = |ms: u64| glint_ipc::monotonic_ns(t0 + Duration::from_millis(ms));
        let judge_with = |trace: &[Trace]| {
            let m = marks(t0, &spec);
            let inputs = [press(t0, 100)];
            judge(
                "test".into(),
                &JudgeCfg::new(G, None),
                &Evidence {
                    marks: &m,
                    end: t0 + Duration::from_millis(700),
                    inputs: &inputs,
                    trace,
                    probe: None,
                },
            )
        };
        assert_eq!(judge_with(&[]).missed, 1);
        // The engine said it may have missed it.
        let r = judge_with(&[Trace::MaybeMissed { at_ns: ns(405) }]);
        assert_eq!((r.missed, r.reported), (0, 1));
        assert!(r.passed());
        // The engine was blind the whole time the color surely showed.
        let r = judge_with(&[Trace::Blind {
            from_ns: ns(390),
            to_ns: ns(510),
        }]);
        assert_eq!((r.missed, r.unseen), (0, 1));
        // Blind for only part of it: a real miss.
        let r = judge_with(&[Trace::Blind {
            from_ns: ns(390),
            to_ns: ns(450),
        }]);
        assert_eq!(r.missed, 1);
        // A self-stop the scenario didn't ask for fails the run.
        let r = judge_with(&[Trace::Stopped {
            at_ns: ns(300),
            cause: StopCause::SafetyStop,
        }]);
        assert_eq!((r.missed, r.appearances), (0, 1)); // nothing can click after a stop
        assert!(!r.passed());
    }

    #[test]
    fn the_split_and_the_summary() {
        let t0 = Instant::now();
        let ns = |us: u64| glint_ipc::monotonic_ns(t0 + Duration::from_micros(us));
        let m = marks(t0, &[(0, Absent, Restart), (100, Tight, Draw), (200, Absent, Draw)]);
        let inputs = [press(t0, 100)]; // 100.3 ms
        let trace = [Trace::Fired {
            request_ns: ns(100_150),
            capture_ns: ns(100_170),
            decided_ns: ns(100_172),
            sent_ns: ns(100_180),
        }];
        let r = judge(
            "basic --count 1".into(),
            &JudgeCfg::new(G, None),
            &Evidence {
                marks: &m,
                end: t0 + Duration::from_millis(400),
                inputs: &inputs,
                trace: &trace,
                probe: Some(&ProbeResult::default()),
            },
        );
        let us = Duration::from_micros;
        assert_eq!(
            r.split,
            Some(Split {
                wait: us(150),
                capture: us(20),
                decide: us(2),
                inject: us(8),
                deliver: us(120),
            })
        );
        let s = r.summary();
        assert!(s.contains("appearances 1 · clicks 1 · extra 0 · missed 0"), "{s}");
        assert!(s.contains("response    p50 0.30 ms"), "{s}");
        assert!(s.contains("split       wait 0.150 · capture 0.020"), "{s}");
        assert!(s.contains("verdict     PASS"), "{s}");
    }
}
