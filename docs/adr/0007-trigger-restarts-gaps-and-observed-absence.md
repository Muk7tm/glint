# 0007. Trigger: every restart latches, the hold sets a minimum gap, stalls aren't absence

- **Status:** Accepted
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
- **Absence counts only time that was watched** (fixes 5).
  - Between two consecutive absent checks, at most `max_check_gap` is credited toward `rearm_absent_ms`:
    four check periods (the §5.6 stall threshold), and never less than 2 ms.
  - With no stall the result is the same as before. After a stall, re-arming waits until enough absence has
    actually been observed.
- **Property tests** cover the six invariants over random check streams. They include stalls, flicker, rapid
  blinking, starts, restarts, pauses and stops, and random settings: 2,000 cases per `cargo test`. A separate
  oracle checks the trace, including that an armed Enter fires on the very check it arrives (speed on the
  rising edge).

## Consequences

- **The C1 cost.** An appearance that begins before `rearm_absent_ms` of observed absence after a Start counts
  as already present: no click until it leaves and returns. That matches the §1 definition of an appearance.
- **The stall cost.** A stall during an absence delays re-arming by up to the stall's length. An appearance
  that begins in that window merges with the previous one: a missed click, never an extra one. With the 10–18 ms
  X server stalls measured in Phase 0, that only happens to appearances less than about 60 ms apart. `glint-lab`
  flags stall-coincident trials (ADR 0004).
- **Holds longer than `min_fire_interval_ms`** silently raise the minimum gap. The UI should say so next to
  *Min gap* (Phase 6).
- **Engine work (Phase 2):**
  - map the control word's armed and gate bits onto `start`, `restart`, `pause` and `stop` (C3);
  - service `Hold::due` before stepping the trigger.
