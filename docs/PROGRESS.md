# Progress

Mirrors the checklists in `docs/PLAN.md` §19. An item is ticked only when it is implemented and its tests
pass. Deviations from the plan are listed in `docs/adr/README.md`.

## Status

- **Active phase:** none. Phase 4 (UI in Pulse's style) is complete, and Phase 5 waits for the owner's
  go-ahead. Glint copies Pulse's Slint files and never changes Pulse (ADR 0012). Open question for the owner:
  keep GPL-3.0-or-later, or switch to MIT under Slint's royalty-free license (AboutSlint + a badge).
  All 17 lab scenarios pass in CI (https://github.com/Muk7tm/glint/actions/runs/43426780742) and at full count
  locally (MEASUREMENTS.md §9).
- **Repository:** public at https://github.com/Muk7tm/glint (the owner chose GitHub, public, for CI).
- Phase 2 is complete: the real-desktop exit run passed on 2026-11-13 (`glint-lab basic --real-desktop --count
  100`: 100 clicks for 100 appearances, 0 extra, 0 missed, p50 0.48 ms, p99 0.71 ms; MEASUREMENTS.md §8).
- `cargo-nextest` and `cargo-deny` are installed (2026-11-13).
- **Owner decisions (2026-11-13):** v1.0 timing is judged with no compositor, with X-server-stall trials
  listed separately (ADR 0004); preventing extra clicks is the top priority, so only watched absence re-arms
  and possible misses are reported (ADR 0007, revised).
- **Phase 5 must:** set the check gap from the refresh rate when a compositor runs (ADR 0007), or re-arming slows
  under picom.
- Xvfb, Xephyr, xwininfo and xev are installed (2026-11-13).

## Scaffold

- [x] Workspace layout (§13) with stub crates, `Cargo.toml` and `rust-toolchain.toml` (§14), `LICENSE`
- [x] `CLAUDE.md`, `docs/PROGRESS.md`, `docs/MEASUREMENTS.md`, `docs/adr/`

Not created yet, because nothing can verify them until their phase: `deny.toml` and `.github/workflows/ci.yml`
(Phase 3), the `glint-ui-view` crate with the Slint UI files and `build.rs` (Phase 4, ADR 0002), `assets/` and `packaging/` (Phase 7), and the other docs
listed in §13 (written as their content appears).

## Phase 0 — Measure before building (S)

- [x] `spike-capture`: 1,000,000 `GetImage` calls for a 3×3 patch on the root window and on an app window; record p50/p99/max with and without a compositor; repeat with MIT-SHM. *(Also measured: a raw allocation-free path, ADR 0003.)*
- [x] Server tax: a second client times `GetInputFocus` round trips while the spike checks at 500/s, 2,000/s and max speed. *(+0.45 ms p99 at Balanced on NVIDIA; ADR 0004.)*
- [x] `spike-inject`: an XTest click into a test window; time from sending to arrival. *(Under Xvfb: p50 6 µs, max < 90 µs; defensive release, release from D, impervious grab control and 0 ms hold confirmed. MEASUREMENTS.md §5.)*
- [x] `spike-ui`: Pulse's components in a new 400 px window with the hero card and the close-dialog overlay. *(Pulse's files used in place via a library path; generated view split out, ADR 0002.)*
- **Exit:** numbers recorded in `docs/MEASUREMENTS.md`; the default speed preset and backend confirmed or changed.

## Phase 1 — `glint-core` (M)

- [x] OKLab/OKLCh, HSV and RGB rules; include and exclude colors; hysteresis; achromatic guard. *(HSV/RGB fields and presets, exclusion defaults, gray-pixel guard: ADR 0005.)*
- [x] 2-bit lookup-table builder (rayon) + the exhaustive equality test. *(One build: ~145 ms on 12 threads. The exhaustive test checks 8 rule sets against a separate reference, ~6.5 s.)*
- [x] Sample patterns and pixel agreement. *(k_stay = ⌈k_enter / 2⌉, plus/custom fields, anchor pixel: ADR 0006.)*
- [x] Trigger state machine + the six invariants as property tests. *(Every restart latches, manual-Start-only `fire_if_present_on_start`, gap ≥ hold + 1 ms, stalls aren't absence, Paused state: ADR 0007. 2,000 random cases per run.)*
- [x] Profile and settings types, validation, migrations. *(Defaults, strictness, limits, hotkey syntax: ADR 0008. Appendix B's examples are tested straight from the plan.)*
- [x] `criterion` benchmark for classification (target: under 100 ns for 9 pixels). *(8.4 ns hot, 15.4 ns with cache misses; verdict + trigger step 13 ns; table build 145 ms. MEASUREMENTS.md §7.)*
- **Exit:** `cargo test -p glint-core` is green, including the exhaustive and property tests.

## Phase 2 — Engine MVP (M)

- [x] X connections A–D, capability probe, pixel-format decoding. *(`wire.rs` raw client for A/B, tested byte-for-byte against x11rb's encoders; x11rb for C/D; local display parsing with a canonical name; tests against Xvfb.)*
- [x] Monitor center through RandR; the `screen` backend. *(GetMonitors → CRTCs → root fallback; primary falls back to the first monitor (C9); GetImage over the raw path; tested end to end on Xvfb.)*
- [x] XTest injector (mouse and keys), timed release, defensive release, panic hook, signal handling. *(Injector on raw connection B, impervious; modifiers in one write; releases every button, modifier and profile key (C10); panic hook on D; SIGTERM/SIGINT/SIGHUP; all tested on Xvfb.)*
- [x] Hot loop, speed presets, control word, epoch restarts, event ring, telemetry thread. *(Control word with armed and gate bits (C3); retired profiles freed by the controller, never the hot thread; allocation-free, checked by assert_no_alloc in debug builds (proven to abort on a deliberate allocation); Xvfb tests: one click per appearance, short dropouts, arm-while-present, gate, safety stop, ~2,000 checks/s.)*
- [x] IPC server (newline-delimited JSON) and `glint-ctl`. *(Same-user socket, 256-message client queues, telemetry and live streams; `glintd` startup per §4.3 with exit code 3 for a second engine; `glint-ctl` incl. `release-all` without an engine; C3, C9, C10, L2 and `get_profile`: ADR 0009. Tested end to end on Xvfb.)*
- [x] Single-instance lock; settings store with debounced atomic writes. *(flock keyed by the canonical display (C6); 300 ms debounce, temp file + fsync + rename; `.bak` on migration; unreadable files reported and left alone; persistence across restarts tested on Xvfb.)*
- [x] Hotkeys through XI2 raw events (F7/F8/F6) with the XTEST-source filter. *(F8 disarms and releases from the X-events thread without waiting for the controller; toggle and hold modes; F6 samples on connection C; keymap changes followed. Tested end to end on Xvfb with keys typed "on" Xvfb's own keyboard device.)*
- **Exit:** from the command line alone, Glint clicks once per appearance on a real desktop, and `basic`, `near-miss` and `arm-while-present` pass under Xvfb. *(Met 2026-11-13: 100 of 100 on the owner's desktop, 0 extra, MEASUREMENTS.md §8.)*

## Phase 3 — Lab and CI (M)

- [x] `glint-lab`: test window, timing, report, every correctness scenario. *(17 scenarios; `move`, `focus` and `ui-overlap` wait for window mode in Phase 5. A trace stream from the engine (every click with its stage times, possible misses, blind gaps, self-stops), a stall probe, and a judge that replays the drawing through the trigger's rules with time bounds: ADR 0010. `cargo test` runs all 17 in about 35 s, and they pass with 8 CPU-burning loops alongside. The `stall` scenario found a trigger bug: a check reads the screen between its request and its reply, and timing it by the reply alone could hide a possible miss or stretch an absence (ADR 0011, fixed, with the ground-truth test now modelling replies).)*
- [x] GitHub Actions: fmt, clippy, nextest, exhaustive test, Xvfb integration, cargo-deny. *(`.github/workflows/ci.yml`: four jobs on ubuntu-24.04, about 5 minutes; nextest runs the exhaustive test alone and two sandboxes at a time; the optional picom job passes too. The first run found that the raw X connection needed an Xauthority file; fixed.)*
- **Exit:** every scenario in §16 that needs neither real hardware nor window mode passes in CI (`move`, `focus` and `ui-overlap` are Phase 5's exit, ADR 0010). *(Met 2026-11-13: CI run 43426780742, all four jobs green.)*

## Phase 4 — UI in Pulse's style (M)

- [x] Copy Pulse's theme and components into `glint-ui-view` (Pulse's repository untouched, ADR 0012). *(`ui/pulse/theme.slint` and `components.slint`, copied unchanged from Pulse at commit e4ccdaa with an origin note; `glint-ui-view` holds the generated code under `deny`, `glint-ui` stays `forbid` (ADR 0002). The window renders in Pulse's style on Xvfb. `deny.toml` ignores four "unmaintained" notices deep in Slint 1.18.1's dependencies.)*
- [x] Hero card, COLOR, CLICKING and GAME WINDOW cards; the *Quit Glint* button. *(One file per card in `ui/cards/`, Pulse's shell and state pattern (`state.slint` globals), a Swatch and a color field as Glint's own pieces, More options with the ENGINE card's close policy. `bind.rs` maps profile and settings to the window and back, with unit tests; until item 3 the window shows the defaults.)*
- [x] Connect to or spawn the engine, handshake, subscriptions, live swatch. *(`client.rs`: a reader thread blocked on the socket and a writer thread, so the window never waits on the engine and wakes only for messages; it starts `glintd` when none answers and reports at once if it exits. The hero button becomes *Restart engine* when the engine is gone. A new `get_settings` request feeds the hotkey and close-policy rows. Checked end to end on Xvfb: the window started its engine, showed an imported profile, Stop while watching, the live swatch "#3BD16F · match", and the restart button after a `kill -9`.)*
- [x] Close dialog and policies (Ask / Keep running / Quit), engine-crash handling. *(The close button and *Quit Glint* follow the policy; *Keep running* detaches and *Quit Glint* quits the engine, each written before the window exits. Without an engine the hero offers *Restart engine*. The engine now sends the live swatch only while watching, and the window drops it while minimized. Checked on Xvfb with Openbox: Keep running left the engine watching and a new window reconnected to it; Quit Glint ended both; the window used 10–30 ms of CPU per 5 s in every state.)*
- [x] Color picking with the magnifier, *Sample center*, key picker. *(The engine grabs the pointer with a crosshair and streams 11 × 11 pixels around it, about 30 frames a second; a left click picks, Escape or another button cancels; refused while watching. The window shows the magnifier at ×8 with the center cell outlined. F6 (*Sample center*) and a pick set the color with the chosen shades. The key pickers store X key names, with keycodes from the X keymap. Tested end to end: `glintd.rs` picks, cancels and checks the grab swallows the click; on Xvfb the window picked #3BD16F, captured F9 and `e`, and saved Loose shades and a new hold.)*
- **Exit:** everything in the main window works without the CLI, and the UI uses about 0% CPU while hidden. *(Met 2026-11-13: every control checked on Xvfb against a real engine; 10–30 ms of CPU per 5 s in every state; CI green, run 43436946481.)*

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
