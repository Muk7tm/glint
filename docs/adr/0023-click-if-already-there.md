# 0023. *Click if it's already there when I start*: a switch for `fire_if_present_on_start`

- **Status:** Accepted (owner request)
- **Date:** 2026-11-14
- **Plan sections:** §5.5, §9, §12.3, §16

## Context

Every start begins latched, so a color that is already showing gets no click until it leaves and comes back (R7,
ADR 0007). The owner noticed that this makes Glint look broken when they start it while the color is already
there. They asked for an option to click such a color once, at the start of an activation only.

The plan already has this option: `fire_if_present_on_start` in the profile's `[trigger]`, off by default.
ADR 0007 limits it to a manual Start:
- the Start button, the start hotkey (including each press of *Hold to watch*), a `start` request, or
  `--start-on-launch`;
- a restart never fires: a move or resize, a profile edit, the window coming back to the front.

The trigger and the engine already implement it, with property tests in `glint-core`. Two pieces were missing:
- the window had no switch for it;
- nothing tested it end to end. The lab's judge treated every start as latched.

## Decision

- **A switch in CLICKING**, under *Start/stop key*: *Click if it's already there when I start*. It edits the
  active profile's `fire_if_present_on_start`. The default stays off.
- **When on, a note says it applies only when you start it.** With a keyboard key as the action, the note adds
  that for the next click the color has to leave and come back. With a mouse button, it says to start with the
  key instead: a mouse click lands at the pointer, which is on Glint's own window when you press *Start* there.
- **The lab's judge tells a manual start from a restart.** `Change::Start` arms at once when the profile has the
  option on. `Change::Restart`, for a profile edit while started, always latches.
- **A new scenario, `fire-on-start`**, runs with the option on. Each round:
  1. a start while the color shows gets one click at once, and no second one while it stays;
  2. the color leaves and comes back: one click;
  3. a profile edit while it shows restarts watching: no click.

## Consequences

- R7's "no click when you start watching while the color is already showing" holds unless the user turns the
  switch on. Restarts still never fire, so moving the window, editing the range or coming back to the front
  can't click.
- With *Hold to watch*, every press that finds the color showing clicks once.
- `arm-while-present` still covers the default, and `fire-on-start` the option. In `cargo test` it has 3
  rounds: 6 appearances, 6 clicks.
