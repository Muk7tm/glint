# 0019. More options: what the plan left open about its three panels

- **Status:** Accepted
- **Date:** 2026-11-13
- **Plan sections:** §10, §12.4, §12.9, §17, §19 (Phase 6)

## Context

§12.4 draws the DETECTION, COLOR RANGE and ENGINE panels, and §12.9 describes *Record 5 s* and *Check hex*.
Building them raised questions the plan doesn't answer:

- **Slint has no file dialog or clipboard API.** *Import*, *Export* and *Copy diagnostics* need both.
- **"Run latency test" doesn't say where it runs.** The lab's real-desktop mode needs the engine's display
  lock, so it can't run beside the engine the window is talking to.
- **The pattern row** shows Pixel, 3×3, 5×5 and Custom. The engine also has 2×2 and Plus, and a custom
  pattern's offsets can't be edited in a row of buttons.
- **The live "Response" line:** telemetry measures the read (capture) and the decision, not the whole
  response. The whole response needs the lab.
- Found while building:
  - *Sample center* read the monitor's center even in window mode.
  - *More options* was saved as open or closed, but never reopened that way.

## Decision

- **DETECTION:**
  - *Pattern* offers Pixel, 2×2, 3×3, 5×5 and Plus. A custom pattern selects none; its offsets live in the
    profile file.
  - A new pattern starts from its own *Must match*.
  - *Checks* shows the rate and warns about Max, and *Gone after* warns under 15 ms.
  - *Read from* names the backend the engine chose ("now: window-buffer").
- **COLOR RANGE:**
  - The model, and the main color's tolerances, in the window's units: OKLCh L and C ×100, hue in degrees;
    HSV hue in degrees, S and V ×100; RGB one value. Only an edited tolerance is written, so the others keep
    their exact values.
  - The shade and hue strips have 16 cells each, spanning 2.2× the range (at least 0.12 of lightness or 24°
    of hue). The engine classifies them: full color starts an appearance, half keeps one going, 25% neither.
  - *Check hex* takes a typed, recorded or sampled color, says what it counts as, and offers *Also match*
    (the main color's shades) and *Never match* (Tight, as §5.4's default for exclusions).
  - *Also match* and *Never match* list those colors, each with an ×.
- **`record_palette`** reads the anchor pixel every 4 ms from the drawable the hot thread reads. It groups
  colors by their top 5 bits per channel, names each group by its most frequent exact color, and returns
  at most 12, most frequent first. Clicking a recorded swatch puts it in *Check hex*.
- **ENGINE:**
  - *Real-time* shows the priority and where the hot thread runs.
  - *Response* shows checks per second, read p50 and p99, and clicks, at 2 Hz and only while the panel is
    open.
  - *Profile* lists the engine's profiles and switches between them.
  - *Copy diagnostics* copies the engine's §17 text block plus the window's version line. It goes through
    an invisible read-only `TextInput` (`select-all`, `copy`).
  - *Import* and *Export* use the desktop's file dialog through xdg-desktop-portal's FileChooser, over
    `zbus`, which Slint's accessibility stack already brings in. Export writes the active profile as TOML.
    Import loads a file and makes it the active profile, as `glint-ctl profile import` does.
- **The latency test** runs `glint-lab basic --count 200` in a test X server of its own, which never
  touches the desktop. It needs Xvfb, and shows "last: 200 runs · 0 extra · 0 missed · p50 … · p99 … ms".
- **Fixes:** *Sample center* reads the watched spot in both modes, like the hot thread. *More options*
  reopens as it was left.

## Consequences

- On a desktop without xdg-desktop-portal, *Import* and *Export* say so. `glint-ctl profile
  import/export` still works.
- The latency test measures the engine in a test X server: correctness and Glint's own speed, not the
  desktop's. The live *Response* line and `glint-lab --real-desktop` cover the real desktop.
- The import, export and latency-test paths have unit tests for their parsing, not end-to-end tests. Their
  dialogs and the lab are external programs. The latency test and *Copy diagnostics* were checked by hand
  on Xvfb.
