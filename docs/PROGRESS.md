# Progress

Mirrors the checklists in `docs/PLAN.md` §19. An item is ticked only when it is implemented and its tests
pass. Deviations from the plan are listed in `docs/adr/README.md`.

## Status

- **Active phase:** Phase 6 (performance and advanced detection), started 2026-11-13: all four items are done; the exit run on the owner's hardware is next (`glint-lab basic --count 10000 --real-desktop`, no compositor, about 1 h 40 min). Phases 4 and 5 are complete. Glint copies Pulse's Slint files and never changes Pulse (ADR 0012). Glint is MIT since 2026-11-13 (owner decision; the About
  dialog shows AboutSlint, ADR 0016).
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

- [x] Pick window, client lookup, identity by `WM_CLASS`, automatic re-acquiring. *(glint-x11 `windows.rs`: the client carrying `WM_STATE`, its frame, class, title, PID, rectangle, the window list and the active window, tested on Xvfb. The engine's `Tracker` (`target.rs`) follows the picked window; `pick_window` uses the crosshair and refuses the desktop and Glint's own window. Checked on Xvfb with Openbox: picked, in front, closed ("waiting for it to open"), and re-acquired on reopening without starting.)*
- [x] Tracking through `StructureNotify`; epoch bump on move and resize. *(The tracker follows the client and its frame; a move or resize re-places the capture and restarts latched. The lab's `move` scenario moves the window while the color shows and resizes it so an off-center patch lands under the new center: no click from either.)*
- [x] *Only click in this window* (EWMH + fallback) with "In front" / "Not in front". *(`_NET_ACTIVE_WINDOW` when the window manager publishes it, focus events otherwise; out of front pauses, back in front restarts latched. Checked on Xvfb with Openbox (EWMH) and by the lab's `focus` scenario without a window manager: no click into the other window, none on refocus.)*
- [x] `window-buffer` backend + self-check; compositor detection; XWayland banner; stop on screen lock; UI-overlap warning. *(The capture reads the picked window itself when the profile asks for it, or automatically in window mode under a compositor, after the self-check (20 reads over 200 ms, once per window). A failure falls back to the screen with a warning that says why. The window-buffer backend keeps the base check gap (ADR 0014). The compositor is followed live through XFixes: under one, screen mode's check gap becomes most of a refresh, and the Watch row says screen mode sees changes one frame late. The XWayland banner and the hint were checked on Xvfb. MIT-SCREEN-SAVER stops watching on lock; D-Bus lock detection moves to Phase 6 (ADR 0014). The UI-overlap warning turns the live swatch off while the window covers the spot. `glint-lab --backend window-buffer`: `move` and `focus` pass on Xvfb (in `cargo test`) and under picom (MEASUREMENTS.md §10). On the owner's desktop, Sober passed the blank test, but the comparison hadn't run (Sober wasn't in front), and comparing at the same instant can't work in a moving scene under a compositor. The comparison now allows for the compositor's delay and runs when the window comes in front; `glint-ctl status` says why (ADR 0014).)*
- **Exit:** `move`, `focus` and `ui-overlap` pass, and the window-buffer backend is verified on your desktop with Sober. *(Met 2026-11-13: the three scenarios pass in CI (run 43458041321), also with the window-buffer backend and under picom. With Sober, the self-check matched 20 of 20 reads, and a recording of 113 appearances found 108 presses, no double press, and 5 unpressed appearances all explained by design (MEASUREMENTS.md §10).)*

## Phase 6 — Performance and advanced detection (M/L)

- [x] rtkit real-time priority, timer slack, prefault and mlock, optional CPU pinning; screen lock over D-Bus. *(`rt.rs`: rtkit makes the hot thread SCHED_RR 10, and puts it back to normal under Max or with `off`; `RLIMIT_RTTIME` with a SIGXCPU fallback; `required` refuses Start without real-time; 1 ns timer slack; pinning to a core; memory locked only with an unlimited `RLIMIT_MEMLOCK`; the LUT build prefaults. `state` reports the priority and CPU, and `glint-ctl status` prints them. The screen lock also follows D-Bus `ActiveChanged` signals. Tested: the kernel shows SCHED_RR 10 on core 0, normal under Max, the other threads normal; a lock on a private D-Bus stops watching. Test sandboxes run with real-time off. No direct `RLIMIT_RTPRIO` path: no safe `sched_setscheduler` (ADR 0017).)*
- [x] `screen-shm` backend; `damage` backend with its self-healing fallback. *(`screen-shm`: a memfd segment attached on connection C, filled by raw `ShmGetImage` on A; chosen when asked or for patterns over 32×32; identical pixels to the socket on Xvfb; the lab's `basic` passes through it. `damage`: the object lives on connection A, reports are noted in the raw packet loop and cleared before each check, the hot thread sleeps in `poll()` between reports and 10 ms safety checks, the trigger's gap is 15 ms, and three unannounced changes in a minute switch to polling for good with a warning. Opt-in only (ADR 0018). Tested on Xvfb: wakes for changes over the spot only; `basic`, `flicker` and `rapid` pass with it, `rapid` also under picom.)*
- [x] *More options*: Detection, Color range (shade strips, exclusions, *Record 5 s*, *Check hex*), Engine (diagnostics, latency test, profiles). *(DETECTION: pattern, Must match, checks, Gone after, Min gap, Safety stop, Read from, offset. COLOR RANGE: model and tolerances, shade and hue strips classified by the engine, Also/Never match lists, Check hex, Record 5 s, Sample center. ENGINE: real-time, live read times, a latency test in a test X server, profiles, Copy diagnostics, Import and Export through the desktop's file dialog. New requests `record_palette` and `diagnostics` (`glint-ctl diag` prints the text). Sample center now reads the window's center in window mode, and More options reopens as left (ADR 0019). Checked on Xvfb: every row edits the profile; strips, Check hex, Never match and its removal, a recording, the clipboard, and a 200-run latency test with 0 extra.)*
- [x] *Capture* hotkey mode. (*Hold to watch* exists since Phase 2, ADR 0013.) *(The hotkeys are grabbed on every lock-key combination, with modifier masks from the server's mapping; the engine gives the keyboard or pointer back the moment a grab fires, so Hold to watch keeps the game's input (ADR 0020). A switch in CLICKING with the trade-off. Tested on Xvfb for F7 and Mouse5, and in the engine with Hold to watch.)*
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
