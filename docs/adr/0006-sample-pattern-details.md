# 0006. Sample patterns: k_stay = ⌈k_enter / 2⌉, plus and custom fields, the anchor pixel

- **Status:** Accepted
- **Date:** 2026-11-13
- **Plan sections:** §5.1, §5.3, Appendix B

## Context

- **The `k_stay` formula contradicts the plan's own table.** §5.3 derives `k_stay = ⌈0.6 × k_enter⌉`, but the
  table next to it says:

  | Pattern | `k_enter` | Table `k_stay` | ⌈0.6 × k_enter⌉ | ⌈k_enter / 2⌉ |
  | --- | --- | --- | --- | --- |
  | Pixel | 1 | 1 | 1 | 1 |
  | Center 2×2 | 3 | 2 | 2 | 2 |
  | Square 3×3 | 5 | 3 | 3 | 3 |
  | Square 5×5 | 13 | 7 | **8** | 7 |
  | Plus | 4 | 2 | **3** | 2 |

- **The plus and custom patterns have no profile fields.** The plus pattern's distance `d` has no field or
  default. The custom pattern has no field for its offsets, no limit on how far they reach, and no default
  `k_enter`.
- **"Anchor on the 2×2 block" isn't precise enough to implement.** §5.1 says that with an even width or height,
  "patterns anchor on the 2×2 block around" the center. It doesn't say which pixel a 3×3 pattern centers on.

## Decision

- **`k_stay`.** When the profile doesn't set it, it is ⌈k_enter / 2⌉, which matches every row of the table. A
  missing `k_enter` takes the table's value. A custom pattern's default `k_enter` is a majority of its pixels,
  ⌊n / 2⌋ + 1, which also matches the table for every square pattern.
- **Fields.**
  - `plus_distance`: the arm length for `pattern = "plus"`, 1–31, default 2.
  - `offsets = [[dx, dy], …]`: for `pattern = "custom"`, 1–64 distinct offsets, each within ±31 px of the
    anchor.
  - Both fields are kept but ignored under other patterns, like the tolerances of other color models
    (ADR 0005).
- **The anchor pixel.** It is `(x + ⌊w/2⌋, y + ⌊h/2⌋)` in the monitor or window.
  - For an odd size it is the exact center pixel.
  - For an even size it is the pixel just right of and below the true center. Center 2×2 covers the four
    pixels around the center, and the odd-sized patterns center on the anchor.
  - The X/Y offset moves the anchor.
- **Capture.** A pattern captures its bounding rectangle, so a plus pattern with arm length `d` reads
  (2d + 1)² pixels and counts 5 of them. The ±31 px reach keeps every capture within 63×63 pixels.
- **Placement.**
  - When an offset pushes the rectangle past an edge, it is moved back inside, and the placement reports
    that it was clamped. Validation rejects such an offset when the profile is applied (§11); a target that
    shrinks later is clamped.
  - An area smaller than the rectangle is an error ("Window too small", §5.1).

## Consequences

- The 5×5 and plus patterns keep the hysteresis the table intended: 7 of 25 and 2 of 5 to stay present.
- `glint-core::pattern` implements all of this, with tests for the table, invalid patterns, placement on even,
  odd and second monitors, and a property test that the verdict follows the pixel counts.
