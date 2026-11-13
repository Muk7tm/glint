# Progress

Mirrors the checklists in `docs/PLAN.md` §19. An item is ticked only when it is implemented and its tests
pass. Deviations from the plan are listed in `docs/adr/README.md`.

## Status

- **Active phase:** Phase 1 (`glint-core`). Phase 0 is done except `spike-inject`, which waits for Xvfb/Xephyr.
- **Waiting on the user:** install Xephyr and Xvfb, needed for `spike-inject` and every input-injecting run
  before Phase 2's exit:
  `sudo pacman -S --needed xorg-server-xephyr xorg-server-xvfb xorg-xwininfo xorg-xev`

## Scaffold

- [x] Workspace layout (§13) with stub crates, `Cargo.toml` and `rust-toolchain.toml` (§14), `LICENSE`
- [x] `CLAUDE.md`, `docs/PROGRESS.md`, `docs/MEASUREMENTS.md`, `docs/adr/`

Not created yet, because nothing can verify them until their phase: `deny.toml` and `.github/workflows/ci.yml`
(Phase 3), the `glint-ui-view` crate with the Slint UI files and `build.rs` (Phase 4, ADR 0002), `assets/` and `packaging/` (Phase 7), and the other docs
listed in §13 (written as their content appears).

## Phase 0 — Measure before building (S)

- [x] `spike-capture`: 1,000,000 `GetImage` calls for a 3×3 patch on the root window and on an app window; record p50/p99/max with and without a compositor; repeat with MIT-SHM. *(Also measured: a raw allocation-free path, ADR 0003.)*
- [x] Server tax: a second client times `GetInputFocus` round trips while the spike checks at 500/s, 2,000/s and max speed. *(+0.45 ms p99 at Balanced on NVIDIA; ADR 0004.)*
- [ ] `spike-inject`: an XTest click into a test window; time from sending to arrival. *(Built and refuses display :0; waiting for Xvfb/Xephyr to run.)*
- [x] `spike-ui`: Pulse's components in a new 400 px window with the hero card and the close-dialog overlay. *(Pulse's files used in place via a library path; generated view split out, ADR 0002.)*
- **Exit:** numbers recorded in `docs/MEASUREMENTS.md`; the default speed preset and backend confirmed or changed.

## Phase 1 — `glint-core` (M)

- [x] OKLab/OKLCh, HSV and RGB rules; include and exclude colors; hysteresis; achromatic guard. *(HSV/RGB fields and presets, exclusion defaults, gray-pixel guard: ADR 0005.)*
- [ ] 2-bit lookup-table builder (rayon) + the exhaustive equality test.
- [ ] Sample patterns and pixel agreement.
- [ ] Trigger state machine + the six invariants as property tests.
- [ ] Profile and settings types, validation, migrations.
- [ ] `criterion` benchmark for classification (target: under 100 ns for 9 pixels).
- **Exit:** `cargo test -p glint-core` is green, including the exhaustive and property tests.

## Phase 2 — Engine MVP (M)

- [ ] X connections A–D, capability probe, pixel-format decoding.
- [ ] Monitor center through RandR; the `screen` backend.
- [ ] XTest injector (mouse and keys), timed release, defensive release, panic hook, signal handling.
- [ ] Hot loop, speed presets, control word, epoch restarts, event ring, telemetry thread.
- [ ] IPC server (newline-delimited JSON) and `glint-ctl`.
- [ ] Single-instance lock; settings store with debounced atomic writes.
- [ ] Hotkeys through XI2 raw events (F7/F8/F6) with the XTEST-source filter.
- **Exit:** from the command line alone, Glint clicks once per appearance on a real desktop, and `basic`, `near-miss` and `arm-while-present` pass under Xvfb.

## Phase 3 — Lab and CI (M)

- [ ] `glint-lab`: test window, timing, report, every correctness scenario.
- [ ] GitHub Actions: fmt, clippy, nextest, exhaustive test, Xvfb integration, cargo-deny.
- **Exit:** every scenario in §16 that doesn't need real hardware passes in CI.

## Phase 4 — UI on Pulse's kit (M)

- [ ] Extract Pulse's theme and components into the shared kit; Pulse stays pixel-identical.
- [ ] Hero card, COLOR, CLICKING and GAME WINDOW cards; the *Quit Glint* button.
- [ ] Connect to or spawn the engine, handshake, subscriptions, live swatch.
- [ ] Close dialog and policies (Ask / Keep running / Quit), engine-crash handling.
- [ ] Color picking with the magnifier, *Sample center*, key picker.
- **Exit:** everything in the main window works without the CLI, and the UI uses about 0% CPU while hidden.

## Phase 5 — Window mode and guards (M)

- [ ] Pick window, client lookup, identity by `WM_CLASS`, automatic re-acquiring.
- [ ] Tracking through `StructureNotify`; epoch bump on move and resize.
- [ ] *Only click in this window* (EWMH + fallback) with "In front" / "Not in front".
- [ ] `window-buffer` backend + self-check; compositor detection; XWayland banner; stop on screen lock; UI-overlap warning.
- **Exit:** `move`, `focus` and `ui-overlap` pass, and the window-buffer backend is verified on your desktop with Sober.

## Phase 6 — Performance and advanced detection (M/L)

- [ ] rtkit real-time priority, timer slack, prefault and mlock, optional CPU pinning.
- [ ] `screen-shm` backend; `damage` backend with its self-healing fallback.
- [ ] *More options*: Detection, Color range (shade strips, exclusions, *Record 5 s*, *Check hex*), Engine (diagnostics, latency test, profiles).
- [ ] *Hold to start* mode; *Capture* hotkey mode.
- **Exit:** on your hardware, `glint-lab basic --count 10000` at Balanced shows p99 ≤ 2 ms and max ≤ 10 ms, with 0 extra and 0 missed.

## Phase 7 — Packaging and release (S/M)

- [ ] Static musl engine, baseline-glibc UI, AppImage, AUR, `.deb` and `.rpm`.
- [ ] `.desktop` file, icon, optional autostart.
- [ ] README, TUNING (compositor settings, rtkit, CPU governor), TROUBLESHOOTING.
- [ ] `LICENSE` in every package and the AppImage; license line and source URL in `--version` and diagnostics.
- [ ] Hardware matrix + 60-minute soak.
- **Exit:** the v1.0 definition of done below.

**Definition of done for v1.0:** every lab scenario passes 10,000 trials on at least two GPU vendors; engine
response p99 ≤ 2 ms at Balanced and never above 10 ms; zero extra clicks and zero missed appearances; UI idle
CPU about 0% while hidden; engine CPU at Balanced measured and documented (budget ≤ 10% of one core); every
stuck-input test passes (SIGTERM, SIGINT, panic, SIGKILL + restart).
