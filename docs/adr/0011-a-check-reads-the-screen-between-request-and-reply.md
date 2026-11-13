# 0011. A check reads the screen somewhere between its request and its reply

- **Status:** Accepted
- **Date:** 2026-11-13
- **Plan sections:** §5.5, Appendix A.1–A.2

## Context

`glint-lab`'s `stall` scenario (ADR 0010) failed under heavy CPU load: about one run in two had an appearance with
neither a click nor a reported possible miss. ADR 0007's promise is that a miss is never silent, so this was a bug.

The trace showed what happened:

1. The color shows, and the engine is latched on it. A check sends its request at 100 ms. The server reads the
   screen at once, still green, and answers.
2. The engine freezes before it reads the reply. In the lab this is SIGSTOP; on a real machine it is the scheduler
   or a page fault.
3. The color leaves at 115 ms. The engine resumes and reads the old green frame at 131 ms.
4. The trigger timed every check by its reply, so it took the color as *seen at 131 ms*. That reset its
   "last seen" time and cleared its "was blind" flag, although the frame was 31 ms old.
5. The color comes back at 165 ms. It was gone for 50 ms, so this is a new appearance. The engine had watched only
   34 ms of absence, so it rightly didn't click. But by its own clock the color had last been seen 34 ms earlier,
   not 65, so it didn't report a possible miss either.

The same mistake also works in the dangerous direction, toward an extra click:
- A reply that comes back up to about 1.5 ms late doesn't look like blind time.
- If the last absent check's reply is that late, timing the absence by replies stretches it by that much.
- So an absence of 38.5–40 ms can measure 40 ms or more, re-arm, and click a second time within one appearance.

The ground-truth property test (ADR 0007) didn't catch either case, because its simulated checks read the screen at
the instant they returned.

## Decision

- **Both times per check.** The trigger takes `step_between(sent, back, verdict)`: the server read the screen
  somewhere in that window. The hot loop passes the request and reply times it already measures. `step(now, v)`
  remains as `step_between(now, now, v)` for tests.
- **Each comparison takes the bound that can't stretch a watched absence or hide a possible miss:**
  - **Blind:** two checks can have read the screen as far apart as this reply minus the previous request.
  - **Re-arm:** an absence counts as watched from its first check's *reply* to the current check's *request*.
  - **Last seen:** the color counts as seen last at that check's *request*. A stale frame then counts as old, and
    a later return is reported.
  - **Confirmation:** `confirm_min` likewise counts from the first Enter check's reply.
- **Where certainty ends.** A re-arm is now certain only after *Gone after* plus two check gaps of absence (44 ms
  at the defaults). That is because what was surely watched can fall short of the truth by one check gap at each
  end.
  - Absences in that band may be missed without a report.
  - Absences shorter than *Gone after* can never re-arm.
  - The ground-truth test's boundary and the lab judge's slack both use two check gaps.
- **The ground-truth simulation now models the reply.**
  - Each check reads the screen 10–30 µs after its request. Its reply is usually back within 25 µs, sometimes up to
    0.6 ms late, and with stalls up to 40 ms late, delivering an old frame.
  - Clicks and reports belong to the moment the screen was read.
  - With the old timing this simulation fails at once ("silently missed"). Two unit tests pin the two cases above.

## Consequences

- Under 8–10 busy CPU loops, `stall` went from 4 unexplained misses in 6 runs to none in 8. Every miss it shows
  now is one the engine reported.
- On a quiet machine nothing changes: the request and the reply are about 20 µs apart.
- On a busy one, Glint reports possible misses a little more often, and re-arms a little later after very short
  gaps. That is the direction the owner chose (ADR 0007).
