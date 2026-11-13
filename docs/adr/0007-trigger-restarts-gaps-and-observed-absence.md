# 0007. Trigger: every restart latches, the hold sets a minimum gap, stalls aren't absence

- **Status:** Accepted. Point 5 revised on 2026-11-13, after the owner made preventing extra clicks the top
  priority: the first version still had a hole.
- **Date:** 2026-11-13
- **Plan sections:** §1, §5.5, §7, Appendix A.1–A.2

## Context

Implementing §5.5 and Appendix A.1 turned up five problems. Problems 1, 2 and 4 are the C1, C2 and C4 proposals
from the Step 1 report. Problem 5 is new.

1. **Restarts contradict the definition of an appearance (C1).**
   - §1 says an appearance begins only after the color was *observed* absent for `rearm_absent_ms` and
     `rearm_absent_samples` checks.
   - A.1's `begin_watch` goes straight to Idle after one absent check at a (re)start.
   - The failure: the color stays on screen while the window moves, the first capture after the move reads a stale
     composited frame without it, and the next check clicks. That is an extra click in the `move`, `focus` and
     `live-edit` cases.
2. **`fire_if_present_on_start` applies to every restart (C2).** A.1 applies it in `begin_watch`. That would
   click on every move or focus change while the color shows.
3. **Nothing stops a restart from undoing a safety stop.** Invariant 5 depends on the engine never calling
   `begin_watch` after one, but focus changes are the controller's while the safety stop is the hot thread's.
4. **A click can be lost during a long hold (C4).** With a 200 ms hold and the default 25 ms minimum gap, the
   trigger can fire again while the previous press is still held. A.4's `press()` then sends nothing, which
   breaks invariant 6 (one press and one release per FIRE).
5. **A stall can fake an absence.** §5.5 says counting only observed absence "is never wrong in the dangerous
   direction", but A.1 measures absence from the first absent check to the current one, gaps included.
   - Take a color that stays on screen except for two short dropouts 45 ms apart. If an engine stall hides the
     presence between them, the trigger sees two absent checks 45 ms apart and re-arms.
   - The next check clicks a second time in the same appearance. The `stall` scenario (§16) is exactly this
     kind of event.

## Decision

- **Every start and restart begins latched, with a fresh absence count**, whatever the color is doing (fixes
  1). An epoch restart therefore needs no capture to decide; the hot loop just calls `restart()` (A.2 updated).
- **`fire_if_present_on_start` applies to a manual Start only.** A manual Start is the button, the hotkey, a
  `start` request or `--start-on-launch`. With the option on, a manual Start begins Idle; restarts always begin
  latched (fixes 2).
- **Only a manual Start leaves Stopped** (fixes 3).
  - `restart()` and `pause()` do nothing while stopped, so invariant 5 holds inside the trigger whatever the
    engine's threads do.
  - A new **Paused** state covers *Only click in this window*: when the target leaves the front, the trigger
    pauses, and when it returns, a restart resumes it latched.
  - `start()` while already started does nothing.
- **The minimum gap is at least the hold time + 1 ms** (`TriggerCfg::min_gap`, fixes 4). The engine's hot loop
  releases a due hold before acting on a check, so every FIRE finds nothing held. The timing lives in
  `glint_core::trigger::Hold`, which the injector follows. The property tests drive it the way the hot loop
  will.
- **Absence counts only while Glint is watching** (fixes 5).
  - Two checks further apart than the check gap mean Glint was blind in between. The check gap is four check
    periods (the §5.6 stall threshold) and never less than 2 ms. A blind gap starts the absence count over: only
    a run of absent checks with no blind gap counts toward `rearm_absent_ms`. A late first check after a
    (re)start counts as blind too, so `start` and `restart` take the time.
  - The first version of this rule credited a stall with 2 ms and kept the absence watched before it. That still
    allowed a second click: 31 ms watched, then 45 ms blind (the color came back and left again unseen), then
    7 ms watched, made 31 + 2 + 7 = 40 ms. A test now covers exactly that.
  - **The guarantee.** Anything on screen for longer than the check gap can't slip between two watched checks,
    and screen content lasts at least a frame (4.2 ms even at 240 Hz). So a watched run proves the color really
    left, and the trigger can't re-arm inside one appearance.
  - **Never silent.** When blind time *may* have hidden a disappearance long enough to end the appearance, the
    trigger doesn't click and returns `Step::MaybeMissed`. That is the case when the color is back, at least
    `rearm_absent_ms` has passed since it was last seen (or since the (re)start), and Glint was blind in between.
    The engine counts and shows these, and `glint-lab` reports them.
- **Property tests** cover the six invariants over random check streams. They include stalls, flicker, rapid
  blinking, starts, restarts, pauses and stops, and random settings: 2,000 cases per `cargo test`. A separate
  oracle checks the trace, including that an armed Enter fires on the very check it arrives (speed on the
  rising edge) and that a possible miss is reported exactly when blind time allows one.
- **A ground-truth property test** checks what matters most directly. A real on/off color, with every state at least
  2.5 ms long, is sampled about 2,000 times a second, with jitter and random stalls of up to 80 ms.
  Appearances are defined from the true signal, and the test requires:
  - never two clicks in one real appearance;
  - every missed appearance explained: never seen, reported as a possible miss, after a safety stop, or gone
    for barely longer than *Gone after*;
  - no possible-miss reports at all when nothing stalls.

  It runs 1,000 scenarios per `cargo test`; one run of 50,000 also passed.

## Consequences

- **The C1 cost.** An appearance that begins before `rearm_absent_ms` of observed absence after a Start counts
  as already present: no click until it leaves and returns. That matches the §1 definition of an appearance.
- **The stall cost.** A stall during an absence restarts the count, so re-arming waits for `rearm_absent_ms` of
  watched absence after the stall. An appearance that begins before that merges with the previous one: a missed
  click, never an extra one, and the trigger reports it.
  - With the 10–18 ms X server stalls measured in Phase 0 (about once a minute), that can only happen to an
    appearance that starts less than about 60 ms after the previous one ended.
  - Glint's own stalls go away with real-time priority (Phase 6).
- **Compositors.** Under picom, gaps over 2 ms are frequent (Phase 0: about 60 late checks per second). The count
  restarts often, and re-arming can take noticeably longer than *Gone after*.
  - A vsync'd compositor shows each frame for at least one refresh (16.7 ms at 60 Hz), so a shorter gap can't
    hide a frame.
  - Phase 5 (compositor detection) will set the check gap from the refresh rate when a compositor runs. The lab's
    `flicker` and `rapid` scenarios under picom verify it.
  - The owner plays with the compositor off.
- **Holds longer than `min_fire_interval_ms`** silently raise the minimum gap. The UI should say so next to
  *Min gap* (Phase 6).
- **Engine work (Phase 2):**
  - map the control word's armed and gate bits onto `start`, `restart`, `pause` and `stop` (C3);
  - service `Hold::due` before stepping the trigger;
  - turn `Step::MaybeMissed` into an event that diagnostics count and show.
