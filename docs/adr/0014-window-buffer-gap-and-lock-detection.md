# 0014. The window-buffer backend keeps the base check gap; D-Bus lock detection moves to Phase 6

- **Status:** Accepted
- **Date:** 2026-11-13
- **Plan sections:** §5.2, §8, §12.3, §19 (Phases 5 and 6)

## Context

- **Check gap.** Under a compositor, Phase 5 widens the trigger's check gap to most of one refresh (ADR 0007).
  That is safe in screen mode because the compositor shows each frame for a whole refresh. The window-buffer
  backend reads the window's own image *before* the compositor, and the app can change that image at any moment.
  With the wide gap, a short dropout in the app's own image could fit between two checks: a re-arm nobody saw.
- **Screen lock.** §8 asks for MIT-SCREEN-SAVER *plus* `org.freedesktop.ScreenSaver` over D-Bus. The owner's
  desktop (LXQt) has no such service: `busctl --user list` shows no ScreenSaver name. D-Bus needs `zbus`, which
  arrives anyway with rtkit in Phase 6. Code added now couldn't be checked on this desktop.
- **Wording.** "With a compositor, screen mode sees changes one frame late" is wrong in window mode when the
  self-check failed and Glint reads the screen for the window.

## Decision

1. **Window-buffer keeps the base gap.** The check gap is four periods and at least 2 ms whenever the
   window-buffer backend is in use, with or without a compositor. Under picom the lab's `move` and `focus`
   scenarios pass with it, with no possible misses (2 ms gap instead of 15 ms).
2. **The self-check** runs when the backend is chosen: on Start, when the profile changes, when the window is
   found again, and when a compositor starts or stops. It runs once per window (by window id) and takes 200 ms on
   the controller thread, never on the hot thread. A failure is a `window_buffer` warning naming the window and
   the reason. The decision is a pure function with unit tests. Without a compositor, a GetImage on a window reads
   the screen, so its two failure cases appear only under a compositor.
3. **D-Bus lock detection moves to Phase 6**, next to rtkit, with `zbus`. Phase 5 stops on the MIT-SCREEN-SAVER
   notify only.
4. **The window-mode hint** reads "With a compositor, reading the screen sees changes one frame late."
5. **`glint-lab --backend auto|screen|window-buffer`** sets the profile's backend, and the report title shows the
   backend the engine chose.

## Consequences

- A locker that maps its own window (xscreensaver, i3lock, slock) and doesn't turn on the server's screen saver
  doesn't stop Glint until Phase 6, and maybe not then. While locked, the locker covers the watched spot, so the
  color can't show there. The worst case is a click on the lock screen when the locker shows the target color
  at the spot.
- The window-buffer backend checks at the base rate under a compositor, so it costs no more CPU than screen mode.
