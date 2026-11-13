# 0014. The window-buffer backend: its check gap and its self-check; D-Bus lock detection moves to Phase 6

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
- **The self-check's comparison** (§5.2: "the two disagree on more than 10% of samples while the window is in
  front") has two flaws, found on the owner's desktop with Sober:
  - The check runs when the backend is chosen. That usually happens while the owner is setting things up in
    Glint's window, so the game isn't in front and the comparison never ran. With Sober, `glint-ctl status` said
    `window-buffer` after only the blank test.
  - Under a compositor the screen shows each frame up to one refresh *after* the window drew it. That is the
    delay window-buffer removes. In a moving scene, reads taken at the same moment disagree even when the
    window image is right. A game drawing faster than the refresh has frames the screen never shows.

## Decision

1. **Window-buffer keeps the base gap.** The check gap is four periods and at least 2 ms whenever the
   window-buffer backend is in use, with or without a compositor. Under picom the lab's `move` and `focus`
   scenarios pass with it, with no possible misses (2 ms gap instead of 15 ms).
2. **The self-check** runs when the backend is chosen: on Start, when the profile changes, when the window is
   found again, and when a compositor starts or stops. It takes 230 ms on the controller thread, never on the hot
   thread, and the emergency stop doesn't wait for the controller. When it runs again is item 4. A failure is a
   `window_buffer` warning naming the window and the reason. The decision is a pure function with unit tests. Without a compositor, a GetImage on a window reads
   the screen, so its two failure cases appear only under a compositor.
3. **D-Bus lock detection moves to Phase 6**, next to rtkit, with `zbus`. Phase 5 stops on the MIT-SCREEN-SAVER
   notify only.
4. **The comparison allows for the compositor's delay.**
   - The screen is read three more times than the window (23 reads, 230 ms).
   - A window read counts as seen when the screen shows the same color in the same read or one of the next
     three (30 ms).
   - At least 90% seen passes. Otherwise the window image must keep changing while the screen does: a moving
     scene, which can't be matched read for read.
   - A frozen image, or one that changes while the screen doesn't, fails.
   - The check runs again when the window next comes in front while Glint is stopped, if the last one couldn't
     compare (or failed). Never while watching, since republishing restarts. A passed comparison stands for
     that window. A failure warns once, not on every retry.
   - `state` gains `backend_reason`, and `glint-ctl status` prints the backend and why ("its own image matched
     the screen in 20 of 20 reads").
5. **The window-mode hint** reads "With a compositor, reading the screen sees changes one frame late."
6. **`glint-lab --backend auto|screen|window-buffer`** sets the profile's backend, and the report title shows the
   backend the engine chose.

## Consequences

- A locker that maps its own window (xscreensaver, i3lock, slock) and doesn't turn on the server's screen saver
  doesn't stop Glint until Phase 6, and maybe not then. While locked, the locker covers the watched spot, so the
  color can't show there. The worst case is a click on the lock screen when the locker shows the target color
  at the spot.
- The window-buffer backend checks at the base rate under a compositor, so it costs no more CPU than screen mode.
