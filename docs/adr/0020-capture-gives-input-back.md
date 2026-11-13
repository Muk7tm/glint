# 0020. Capture mode gives the keyboard or pointer back at once

- **Status:** Accepted
- **Date:** 2026-11-13
- **Plan sections:** §9, §12.3, §19 (Phase 6)

## Context

§9's optional *Capture* mode passively grabs every hotkey (`XGrabKey`, for each Caps Lock, Num Lock and
Scroll Lock combination), so the focused app doesn't get it. The plan names one trade-off: a passive grab
can't fire while another program holds an active keyboard grab.

There is a second, worse one. When a passive grab fires, it becomes an **active grab of the whole keyboard**
(or pointer, for a button grab) until the key or button is released. Glint has *Hold to watch* (ADR 0013),
and the owner holds Mouse5 the whole time they play. While it's held:
- the game would get no other key, or no mouse input at all;
- Glint's own clicks and key presses, which go through XTEST, would be delivered to Glint's grab instead of
  the game.

So Capture would break exactly the way the owner uses Glint.

## Decision

- **Grab, then let go at once.**
  - The engine grabs the hotkeys on connection C.
  - The moment one fires, the X-event thread receives its core `KeyPress` or `ButtonPress`. It sends
    `UngrabKeyboard` or `UngrabPointer` immediately.
  - The press itself stays swallowed. Every event after it, including the hotkey's own release, reaches the
    app again.
  - Glint still recognizes hotkeys from XInput 2 raw events, which arrive whatever is grabbed. Hold to watch
    keeps working: the release is seen raw.
- **Mouse side buttons** are grabbed with `XGrabButton`.
- **Modifier masks:**
  - Alt, Super, Num Lock and Scroll Lock come from the server's modifier mapping, not assumed to be Mod1,
    Mod4, Mod2 and none.
  - Grabs cover every combination of the lock keys that exist.
- **Grabs follow the settings:** made at startup and on every settings change, and dropped when Capture is
  off. A hotkey another program already holds gets a warning ("…: another program holds it, so the game gets
  it too"), and Glint still hears it through raw events.
- **In the window:** a *Keep the hotkeys from the game* switch in the CLICKING card under *Start/stop key*,
  with the plan's trade-off as its note: "They don't work while another program holds the whole keyboard."

## Consequences

- Between a hotkey's press and Glint's ungrab, well under a millisecond, other input goes to Glint and is
  lost. With Hold to watch, every start latches and needs 40 ms of absence first, so no click is due then.
- The app gets the hotkey's release without its press. Apps treat a lone release as nothing.
- **Tested:**
  - On Xvfb: F7 and Mouse5 are kept from the window, and the next key or click reaches it.
  - In the engine: holding F7 with Capture and Hold to watch watches, the game never gets F7, and it gets
    `a` while F7 is held.
