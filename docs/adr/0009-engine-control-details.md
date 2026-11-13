# 0009. Engine control: armed and gate bits, the display lock, monitor fallback, releasing everything, UI disconnects

- **Status:** Accepted
- **Date:** 2026-11-13
- **Plan sections:** §4.2, §4.3, §4.4, §5.1, §10

## Context

Building the engine in Phase 2 turned four of the Step 1 proposals (C3, C9, C10, L2) into code, and found one
gap in the protocol.

- **C3. One armed bit can't tell a stop from a focus change.** §4.2's control word has a single `armed` bit, and
  the controller bumps the epoch on focus changes. The hot thread clears `armed` on a safety stop. If the
  controller re-arms on "window back in front" before it has seen that stop, the safety stop is undone
  (invariant 5).
- **C9. There may be no primary monitor.** On the reference machine, RandR's primary output (HDMI-0) is
  disconnected, so no active monitor is primary, and §5.1's "the default is the primary" picks nothing.
- **C10. The defensive release misses keys.** §4.3 releases buttons 1, 2, 3, 8, 9 and *the active profile's*
  key. That misses:
  - modifier keys;
  - the key of a profile that was active when the previous engine was killed.

  `glint-ctl release-all` goes through the engine, but a dead engine is exactly when it's needed.
- **L2. Which clients count.** §4.3 stops the engine when "the UI crashes or is killed". `glint-ctl` and
  `glint-lab` also connect and disconnect.
- **A protocol gap.** §10 lists `glint-ctl profile show` and `profile export <name>`, but no request returns a
  profile.

## Decision

- **Control word:** `[armed:1][gate:1][quit:1][reserved:13][epoch:16][x:16][y:16]`.
  - `armed` is set only by a manual Start and cleared by every stop, including the hot thread's own.
  - `gate` says whether clicking is allowed right now: the target window is in front, or *Only click in this
    window* is off.
  - An epoch change maps onto the trigger:
    - armed, after being disarmed: `start`;
    - armed both before and after: `restart`;
    - disarmed: `stop`;
    - armed with the gate closed: `pause`.
  - `quit` ends the hot thread, which releases input before it exits.
- **Monitor:** `"primary"` means the primary monitor, or the first active monitor when none is primary.
- **Releasing everything:**
  - The startup release, the panic hook (connection D) and `release_all` release:
    - every button Glint can press;
    - every modifier key in the live keymap;
    - the key of every profile.
  - When no engine answers, `glint-ctl release-all` connects to the display itself and releases every button
    and every keycode through XTest. A release of something that isn't held does nothing (measured in
    Phase 0).
- **Disconnects:** only a `ui` client that disappears without `detach` stops watching (unless the policy is
  *Always keep running*), and that means stop and release, not exit. `glint-ctl` and `glint-lab` never stop the
  engine by leaving.
- **Protocol:** a new request `get_profile {name?}` returns `profile {profile}`. This is an addition, so
  `PROTOCOL` stays 1.

## Consequences

- The hot thread disarms itself with a compare-and-swap on the control word. A Start that lands between that and
  its next iteration still counts as a manual Start (Appendix A.2).
- Tests:
  - `glint-engine/tests/hot_loop.rs`: the gate;
  - `glint-engine/tests/glintd.rs`:
    - the engine through its socket;
    - `glint-ctl` against it;
    - `release-all` with no engine, releasing a button another process left pressed;
  - `glint-x11`: the injector's releases.
- **C6, the single-instance lock.** A non-blocking `flock` on `glintd-<N>.lock` in the socket directory, held for
  the engine's lifetime and keyed by the canonical display number. So `:0`, `:0.0` and `unix:0` share one lock
  (`lock.rs`, tested with every spelling), and a second engine exits with code 3.
