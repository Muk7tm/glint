# 0005. Color rules: HSV and RGB tolerances, exclusion defaults, and a gray-pixel guard

- **Status:** Accepted
- **Date:** 2026-11-13
- **Plan sections:** §5.4, §11, Appendix B

## Context

§5.4 specifies the OKLCh model completely but leaves four gaps.

1. **HSV and RGB tolerances are undefined.** §5.4 says only "H ± ΔH, S and V within bounds" and "each channel
   ± t". There are no field names, defaults or presets, and Appendix B shows only OKLCh fields.
2. **Exclude colors have no default tolerances.** §5.4 says each has "its own small range".
3. **The achromatic guard only covers gray *targets*.** For a muted but colored target, the chroma range
   `C ± ΔC` reaches down to gray pixels, and a gray pixel's hue is noise. One step in one channel swings it
   around the circle:

   | Pixel | Chroma | Hue |
   | --- | --- | --- |
   | `#808080` | 0.0000 | 89.9° |
   | `#818080` | 0.0012 | 17.2° |
   | `#808180` | 0.0019 | 145.6° |
   | `#808081` | 0.0015 | 286.4° |

   Take the muted green target `#7A8A7E` (C 0.026, h 153°, with Normal tolerances). `#808180` falls inside its wide
   range and `#808081` doesn't, so a flat gray background with a little noise would match in a speckled
   pattern.
4. **HSV saturation is unstable for dark pixels.** `#030101` has a saturation of 0.67 but is visibly black. A gray
   test based on saturation would treat dark noise as colorful.

Also, §11 requires "all tolerances > 0", but §5.4 offers the RGB box for "exact-color matching", which needs a
tolerance of 0.

## Decision

- **Fields.** HSV uses `tol_h` (degrees, the same field as OKLCh's hue), `tol_s` and `tol_v` (0–1). RGB uses
  `tol_rgb` (an integer from 0 to 255; 0 matches only the exact color). Like OKLCh, both are boxes of ± tolerances around the target, so
  the wide range is the tight range × `exit_margin` in every model. Each entry keeps the fields of every model
  and only the profile's model reads them, so switching models back and forth loses nothing.
- **Presets.** OKLCh keeps the plan's values. HSV and RGB get their own:

  | Preset | HSV ΔH · ΔS · ΔV | RGB t |
  | --- | --- | --- |
  | Tight | 4° · 0.08 · 0.08 | 6 |
  | Normal | 7° · 0.15 · 0.15 | 12 |
  | Loose | 12° · 0.25 · 0.25 | 20 |

- **Missing tolerances.** An include color takes the Normal values, and an exclude color takes the Tight values.
- **Gray-pixel guard.** For a colored target, a pixel below the achromatic threshold fails the hue test.
  - In OKLCh that means chroma below 0.02. In HSV it means `(max − min) / 255` below 0.02, which, unlike
    saturation, stays near 0 for dark noise.
  - Gray targets keep the plan's rule: hue is ignored, and only L and C (or S and V) are compared.
  - The threshold is the same for the tight and wide ranges, so the tight range still lies inside the wide range.
- **Defensive compile.** `exit_margin` below 1 is treated as 1, so the wide range can never be the smaller one.
  Validation rejects such values anyway.

## Consequences

- Muted targets no longer match gray noise. Nothing changes for any target whose wide chroma range stays at or
  above 0.02, which includes every saturated color with the presets.
- There is a step at chroma 0.02 between "gray target: hue ignored" and "colored target: grays rejected". It was
  already implied by the plan's guard; the UI's gray warning covers it.
- §11's rule becomes: all tolerances > 0, except `tol_rgb`, which may be 0.
- Tests: `color::rule` (gray guard, hue wrap-around, presets, exclusion, TOML round trip). The exhaustive
  lookup-table test (Phase 1) covers every color against a separate reference implementation.
