# 0018. The `damage` backend: reports on the hot connection, a 15 ms check gap, opt-in only

- **Status:** Accepted
- **Date:** 2026-11-13
- **Plan sections:** §5.2, §5.5, §19 (Phase 6), §21 (open question 6)

## Context

§5.2 describes the backend: a damage object at the bounding-box level; `poll()` on the X socket with a 10 ms
safety timeout; an immediate check and a follow-up about 1 ms later when a report touches the spot; a miss
when a safety check finds a change no report announced; back to polling after three misses in a minute.
Building it showed what the plan doesn't say:

- DamageNotify goes only to the client that created the damage object, so the object must live on the hot
  connection A. The raw client therefore negotiates DAMAGE itself: a client must send `QueryVersion` before
  other DAMAGE requests. It also creates the object from its own id range.
- The X server answers in order. A report that arrives before a `GetImage` reply comes from drawing the
  server did before that read, so the read already shows it.
- A bounding-box report fires only when the damaged area grows. Without clearing it after every report, a
  later change inside the box is never reported.
- A new damage object starts with the whole window damaged (seen on Xvfb). Clients are meant to repaint
  everything then.
- The trigger's rule (ADR 0007) counts an absence only over checks no further apart than the check gap,
  2 ms at Balanced. Event-driven checks are often 10 ms apart, and under polling rules every one of those
  gaps would be blind.

## Decision

1. **Connection A follows DAMAGE.**
   - The raw client negotiates DAMAGE 1.1 once at startup, before the hot thread runs, and takes the first id
     of its own range for the one damage object.
   - It notes DamageNotify events wherever it reads packets: while waiting for a reply, and while draining
     the socket. It records whether any report came in and whether any report's area touched the watched
     rectangle.
2. **Every report is cleared before the next check.** `DamageSubtract` with no regions goes out first, then
   the check. The check is processed after the clearing, so anything the clearing erased is in what it reads.
3. **The wait.** The hot thread waits with `poll()` until one of these:
   - a report touches the spot (check now, and again 1 ms later);
   - the follow-up is due;
   - 10 ms have passed since the last check (a safety check).

   With nothing changing, it sleeps in `poll()`.
4. **Check gap 15 ms**, a safety period and a half. With reports trusted, no report means nothing over the
   spot changed, so the gap isn't blind. `state.check_gap_us`, the telemetry's blind gaps and the lab's
   slack all follow it.
5. **A miss** is a check whose pixels (a checksum of the pattern's rectangle) differ from the previous check,
   when no report announced the change. A change counts as announced by:
   - the report that ended the wait;
   - a report that came in during the check's own read;
   - any report within the previous 3 ms (the report's check and its follow-up).

   Three misses within a minute make the hot thread poll for the rest of its life: the polling check gap and
   the profile's period. It sends `DamageUnreliable`. The controller then warns "This system doesn't report
   screen updates reliably; using regular checks.", republishes without DAMAGE, and never uses it again
   until the engine restarts.
6. **Not watching** (stopped or paused) destroys the damage object, so reports don't pile up unread on the
   server. Watching again creates it, and its whole-window report costs one extra check.
7. **Opt-in only.** `backend = "damage"` reads the screen (`screen + damage`) in both modes, and `auto` never
   picks it.
8. **The lab:** `--backend damage`, and `basic`, `flicker` and `rapid` run with it in `cargo test`.

## Consequences

- **The safety argument rests on reports.** If DAMAGE silently misses changes, a reappearance shorter than
  15 ms inside an absence can go unseen. The absence then counts as watched, and a flickering appearance could
  re-arm and click twice before three misses switch to polling. That would break R1, which the owner ranks
  first. So DAMAGE stays opt-in, and open question 6 ("DAMAGE as the default") should be answered "no"
  unless the real-hardware runs show no misses at all.
- **A Stop arriving during a wait is seen at the next report or safety check,** at most 10 ms later. No click
  can follow it, because the control word is read before every check. The emergency stop releases input on
  its own connection at once.
- **Measured on Xvfb** (`glint-lab basic --count 20 --backend damage`): 20 of 20 clicked, response p50
  0.15 ms. The hot thread woke 0.07 ms after a change, and other clients' round trips rose 0.03 ms at p99.
  Real-hardware numbers come with the Phase 6 exit runs.
