# 0015. *Loose+*: a fourth shades option that takes much lighter and darker shades

- **Status:** Accepted (owner request)
- **Date:** 2026-11-13
- **Plan sections:** §5.4, §12.3

## Context

On the owner's desktop, an appearance of the owner's purple `#8604A0` in Sober often begins **light pink**
(`#C243DE`–`#D051DE`) for 4–28 ms before it turns the purple (MEASUREMENTS.md §10). In OKLCh the pinks keep the
hue (Δh +1° to +5°) and the chroma (ΔC ≤ 0.022), but they are 0.17–0.20 lighter. *Loose*, the widest preset,
allows ±0.16 of lightness, so the pinks fall in the wide range only ("still there"). Glint therefore pressed
when the purple arrived, up to 28 ms after the appearance began. The owner noticed this as "it won't click fast
enough at the start".

The owner asked for the fix to be one of the *Shades* options, either inside *Loose* or as a new option.

## Decision

A fourth preset, **Loose+**: *Loose* with only the lightness widened.

| Model | Loose | Loose+ |
|---|---|---|
| OKLCh (ΔL · ΔC · Δh) | 0.16 · 0.08 · 12° | **0.22** · 0.08 · 12° |
| HSV (ΔH · ΔS · ΔV) | 12° · 0.25 · 0.25 | 12° · 0.25 · **0.35** |
| RGB (t) | 20 | **28** (RGB can't separate lightness) |

- **Why a new option, not a wider *Loose*.** *Loose* keeps its meaning for every other profile. A light pink
  start is particular to some games, and the wider range has a cost: anything else light purple at the spot
  now starts an appearance.
- **Why lightness only.** The measurement shows the pinks differ in lightness alone. Hue still keeps
  neighboring colors out (§5.4, R5).
- **The name.** "Wide" would clash with the *wide range* of hysteresis (`Class::Wide`, §5.4). "Very loose"
  doesn't fit four options in the row. The code calls it `ShadePreset::LoosePlus` (`loose-plus`).

## Consequences

- The owner's profile now uses Loose+ (`tol_l = 0.22`), applied through `glint-ctl profile import`.
- A unit test pins the measured pinks: *Loose* keeps them, *Loose+* starts on them, and hues 23° away stay out.
- With the 1.5 exit margin, the "still there" range reaches ±0.33 of lightness.
