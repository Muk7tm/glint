# 0025. The Area pattern and *Must last*, from the owner's training recording

- **Status:** Accepted (from the owner's recording and request)
- **Date:** 2026-11-14
- **Plan sections:** §5.3, §5.5, §12.4, §16

## Context

The owner recorded 2 min 14 s of a Roblox block-training mode (Sober) with Glint running and asked for every
problem it shows to be fixed, and for settings that work. Glint was watching the window's center, which is the
character's head, with:

- color `#8300AA`, Loose+; a 3×3 pattern, *Must match* 5;
- *Gone after* 40 ms, Hold to watch on Mouse5, the action key `e`.

The game highlights the targeted character in purple; pressing block shows a shield, and a successful block
clears the BLOCK button's cooldown early. MEASUREMENTS.md §13 has the frame-by-frame analysis: 8,068 frames,
the engine's own color rules, and the presses read from the BLOCK button. It found:

1. **Late and missed clicks came from the color.** The head's highlight is `#800080` (hue 328°), 14° from
   `#8300AA`, beyond Loose+'s 12°. Only 53 frames had the 5 tight pixels a click needs. Glint pressed only when
   something tinted the head into range. The owner has since switched to `#800080`.
2. **The double clicks came from a flash after each block.** Purple returned for 1–3 frames (17–50 ms), 67–317 ms
   after the shield replaced it, and Glint took it for a new appearance: 5 wasted presses, 85–117 ms after
   successful blocks. Real appearances lasted at least 4 frames (67 ms).
3. **A 3×3 spot is too small for a moving character.** The head slides off and back as the character dashes,
   turns and blocks. With `#800080`, 21 of 42 targetings would have been pressed more than once, and no *Gone
   after* or confirmation setting fixed both doubles and misses at the 3×3 spot.
4. **An area that covers the character fixes both.** An 8×8 grid over 63×63 px, centered just below the head,
   with 8 of 64 points needed:
   - With *Gone after* 100 ms and the color required for 4 frames: one press for each of the 46 real
     appearances, none on the flashes, 50 ms after the purple appeared.
   - The game gives that time: the ball arrived 217 ms to 3.4 s after the purple.
5. **The window couldn't set either.** A custom pattern lives only in the profile file. `confirm_min_ms` had no
   control and was capped at 50 ms, the length of the longest flash.

## Decision

- **`PatternKind::Area`** (`pattern = "area"`):
  - 64 points at −31, −22, −13, −4, 4, 13, 22 and 31 px on each axis, 63×63 px, within MAX_POINTS and MAX_REACH;
  - *Must match* 8, so `k_stay` 4;
  - the engine reads its rectangle like any pattern's, through shared memory under Auto (over 32×32);
  - it is the sixth option of DETECTION's Pattern row.
- **DETECTION's *Must last*** sets `confirm_min_ms`: 0 (the default, click at once) to 250 ms. The validated range
  grows from 0–50 to 0–250.
- **`glint-lab --pattern`** (pixel, 2x2, 3x3, 5x5, plus, area) runs any scenario with another pattern. A lab test
  runs `basic` and `boundary-noise` with Area.
- **The defaults stay as they were.** 3×3 and no confirmation keep the first click as fast as possible, as §5.5
  wants. The owner's profile gets the measured settings:

  | Setting | Value |
  | --- | --- |
  | Color | `#800080`, Loose+ |
  | Pattern | Area, *Must match* 8 |
  | *Must last* | 60 ms |
  | *Gone after* | 100 ms |
  | *Click if it's already there when I start* | on |

## Consequences

- A flash shorter than *Must last* never clicks, and every click comes that much later. 60 ms clears the
  3-frame flashes (50 ms) and leaves real appearances (67 ms and more) a margin.
- In this game the purple can appear up to 3.4 s before the ball arrives, while the recording's successful blocks
  were pressed at most 470 ms before it. A press the instant the purple appears can come too early for a slow
  ball; Glint can't see the ball's distance. The report to the owner says so.
- An Area pattern reads 63×63 px per check, about 16 KB, through shared memory.
