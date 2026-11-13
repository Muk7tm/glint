# 0013. Hold to watch, typed color codes, and mouse side buttons as hotkeys

- **Status:** Accepted (owner requests)
- **Date:** 2026-11-13
- **Plan sections:** §9, §12.3, §19 (Phase 6)

## Context

After Phase 4 the owner asked for three things:

1. **Hold mode** should keep Glint active only while the hotkey is held, and stop everything the moment it's
   released. The engine already did that, but the window called it "Hold to start", which sounds like holding
   starts it once.
2. **Typing a color code.** The color could only be picked from the screen, or sampled with F6. The plan typed hex
   only into *Check hex*, in *More options*, in Phase 6.
3. **Mouse side buttons as hotkeys.** Pulse allows them, but Glint's hotkeys only listened to keyboard keys.

## Decision

1. **Hold to watch.** The mode is renamed *Hold to watch*. In it, the status line says "Hold F7 to watch". Its
   behavior is unchanged: the press starts watching, and the release stops and releases input.
2. **Typed colors, now.** The COLOR card's color field takes a typed hex code (`#3BD16F`, `3bd16f` or `#3D7`).
   Enter applies it with the chosen shades, and a code that doesn't parse says so.
3. **Side buttons as hotkeys.**
   - `Mouse4` and `Mouse5` (X buttons 8 and 9) can be hotkeys. The engine selects XInput raw button events next
     to the key events.
   - XTEST-sourced buttons are ignored, so Glint's own Back and Forward clicks never count.
   - An action on the same side button as a hotkey is rejected.
   - The start/stop key field takes a side-button click, and lists both side buttons first, as Pulse does.

## Consequences

- Tests cover the matcher and the validation. On Xvfb they also check the raw-button events, and that the engine
  takes `Mouse4` while ignoring XTest's button 8.
- Xvfb's virtual mouse has only three buttons, so a real side button can only be tried on real hardware.
- Phase 6 keeps *Capture* hotkey mode and *Check hex*. *Check hex* still shows whether a code counts, without
  setting it.
