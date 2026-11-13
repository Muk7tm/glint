# 0010. The lab: a trace stream from the engine, judging with time bounds, and what Phase 3 covers

- **Status:** Accepted
- **Date:** 2026-11-13
- **Plan sections:** §10, §16, §19

## Context

Building every §16 scenario in Phase 3 turned up five gaps.

1. **The engine doesn't tell the lab enough.**
   - §16 splits each response into stages "with Glint's fired events". But `fired` is coalesced to at most
     20 per second and carries only a click count and the decide time.
   - Possible misses (ADR 0007) and blind time reach clients only as telemetry counters. So the lab can't tell
     which appearance a reported miss belongs to, which ADR 0004 needs: "the only miss a stall can cause is one
     the engine itself reports".
2. **"One click per appearance" needs a definition the lab can apply.**
   - An appearance ends only after the color is gone for *Gone after* (§1). So the lab must know how long each
     gap really was on screen.
   - A client can't see when the server carried out its drawing, only that it happened after the request went out
     and before the round trip came back. And on a busy machine the lab's own sleeps overrun.
   - A 33 ms flicker gap or a 25 ms `rapid` gap can therefore end up near the 40 ms line by accident.
3. **§16's CI command doesn't fit the lab.** It runs the lab tests under `xvfb-run` with `--run-ignored all`, but
   since Phase 2 every scenario starts its own Xvfb and `glintd`, and none is ignored.
4. **Phase 3's exit asks for every scenario that doesn't need real hardware.** That includes `move`, `focus` and
   `ui-overlap`, which need window mode and the UI. Phase 5's exit already lists exactly those three.
5. **Some scenarios can't run on the real desktop.**
   - `action-matrix` presses keys, which reach whatever window has the focus.
   - `kill-mid-press` leaves input held on purpose.

## Decision

1. **A trace stream.**
   - `subscribe` takes `trace: true`. The engine then sends each `trace` event as it happens, without coalescing:
     - `fired {request_ns, capture_ns, decided_ns, sent_ns}`;
     - `maybe_missed {at_ns}`;
     - `blind {from_ns, to_ns}`: no check came back for longer than the check gap;
     - `stopped {at_ns, cause}`.
   - Times are `CLOCK_MONOTONIC` nanoseconds, which every process on the machine shares (`glint_ipc::monotonic_ns`).
   - The telemetry thread builds the stream only while a client asks for it. The hot loop adds one clock read per
     click, for the decision time.
   - These are additions, so `PROTOCOL` stays 1.
2. **Judging with time bounds.**
   - The lab records every change it makes with two times: its request going out, and its round trip coming back.
     Starts, stops and profile edits get 2 ms more, for the hot thread to pick them up.
   - The judge replays these marks through the trigger's rules with the shortest and longest possible duration of
     each. An appearance must get a click when the gap before it was surely at least *Gone after* plus two check
     gaps, the band where the engine's re-arm is certain (ADR 0011). It may get one when the gap could have fallen
     on either side.
   - An extra click is never excused.
   - A missing click is excused only in these cases, each listed separately in the report:
     - the engine stopped itself;
     - the engine reported a possible miss;
     - the engine was blind for the whole time the color was surely on screen;
     - the color was never surely on screen for 2.5 ms.
   - Clicks slower than 10 ms are flagged, never failed.
3. **The report.**
   - The stages are wait, capture, decide, inject (writing the press) and deliver (the server handling the fake
     input and delivering it to the lab's window).
   - A probe client times a `GetInputFocus` round trip every millisecond. A round trip over 5 ms is an X server
     stall, and trials it overlaps are listed apart from the timing (ADR 0004).
   - `basic` and `soak` watch for 1 s of quiet first, which gives the server tax.
   - Timing is part of the verdict only on the real desktop, and only for `basic` and `shades`.
4. **Sandboxes, not `xvfb-run`.** Each scenario runs in its own Xvfb with its own `glintd` and scratch directories,
   so CI runs the lab tests as ordinary tests.
5. **Phase 3 covers 17 scenarios:** every §16 scenario except `move`, `focus` and `ui-overlap`, which stay with
   window mode in Phase 5.
   - `soak` takes `--minutes`. CI runs it for a few seconds; the 60-minute run is for real hardware before releases.
   - `two-engines`, `action-matrix`, `kill-mid-press` and `action-equals-hotkey` run only in the sandbox.
6. **Scenario details the plan leaves open.**
   - `feedback` lowers the safety limit to 3 clicks per second, so the stop comes within a few clicks on any
     machine. It checks the rule itself against the engine's traced click times, not a fixed deadline: no second
     ever holds more than 3 clicks, every stop comes when a 4th is due within a second, and no click follows a
     stop. Its rounds start more than a second apart, because a Start keeps the last second's clicks in the count
     (the stricter behavior, unchanged).
   - `action-matrix`, `kill-mid-press` and `action-equals-hotkey` check the action, not the re-arm. If the engine
     reports a possible miss instead of clicking (blind time on a busy machine, right after a restart), they show
     the color again, up to three times.
   - Half of `stall`'s gaps are just under *Gone after*. A freeze at either end then makes the real gap long enough
     to call for a click, while the engine has watched too little of it to re-arm. The engine has to report a
     possible miss, and the judge checks that it did.
   - `distractor` draws each shape in one request, so no check can catch a half-drawn frame on the sampled pixels.

## Consequences

- The lab can attribute every click, possible miss and stall to one appearance. The ground-truth property test
  (ADR 0007) and the lab now use the same rule for which misses are acceptable.
- `cargo test` runs all 17 scenarios with small counts in about 35 s, at most 4 sandboxes at once. They pass
  with 8 CPU-burning loops running alongside.
- The lab found a real bug in its first week: a stale frame delivered after a stall could hide a possible miss,
  and a late reply could stretch an absence (ADR 0011).
- A real-desktop run prints the same report, and its timing verdict is the check Phase 6's exit uses.
- The scenarios' pass conditions are now in `glint-lab/src/scenarios.rs` and `judge.rs`, with unit tests for the
  judge's rules.
