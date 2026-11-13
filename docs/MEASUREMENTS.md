# Measurements

Phase 0 (PLAN §19), measured on 2026-11-13 with the spikes in `spikes/`. All times are microseconds unless
marked otherwise.

## Environment

| Item | Value |
| --- | --- |
| `XDG_SESSION_TYPE` | `x11` (no `WAYLAND_DISPLAY`, no XWAYLAND extension) |
| X server | X.Org 21.1.26 (vendor release 12101026), protocol 11.0 |
| Desktop | LXQt with Openbox; no compositor by default (`_NET_WM_CM_S0` unowned) |
| Compositors tested | picom v13 `--config /dev/null`, backends `xrender` and `glx --vsync` |
| GPU / driver | NVIDIA GeForce RTX 3060 (GA106), proprietary driver 620.70.06, no ForceFullCompositionPipeline |
| Display | one monitor, HDMI-1, 1920×1080 @ 60.00 Hz; RandR reports it as **not primary** (the primary output, HDMI-0, is disconnected) |
| Root visual | depth 24, 32 bits per pixel, scanline pad 32, LSB first, TrueColor, masks R `ff0000` G `00ff00` B `0000ff` |
| Extensions | XTEST 2.2 · MIT-SHM 1.2 (fd passing; no shared pixmaps) · RANDR 1.6 · XInput 2.4 · DAMAGE 1.1 · Composite 0.4 · XFIXES 6.0 · MIT-SCREEN-SAVER 1.1 |
| CPU | Intel Core i5-11400F, 6 cores / 12 threads, governor `performance` |
| Kernel | 7.2.15-zen1 (`PREEMPT_DYNAMIC`) |
| Limits | `RLIMIT_MEMLOCK` 8 MiB · `RLIMIT_RTPRIO` 99 · rtkit running |
| Other X clients during the runs | VS Code (this session), Firefox, LXQt panel and desktop, and `apex` (~10% CPU). Xorg itself used ~8% CPU. |

## Method

`spike-capture` opens a 64×64 override-redirect window filled with `#3BD16F` in the top-right corner and reads the
3×3 patch at its centre, either through the root window (the `screen` backend) or from the window itself (the
`window-buffer` backend). Every run checked that the decoded centre pixel was `#3BD16F`; all did.

- **Methods:** `x11rb` is x11rb 0.13.2's `get_image(...).reply()` (Appendix A.3). `raw` is a minimal client that
  writes a fixed 20-byte GetImage request and reads the reply into fixed buffers on a blocking Unix socket
  (connection setup via x11rb-protocol). `shm` is x11rb's `shm_get_image` into a memfd segment.
- **Tight loop:** back-to-back checks; this measures the round trip itself.
- **Paced:** checks at fixed absolute deadlines (sleep, then a 60 µs spin), as the engine will run. Paced
  checks land uniformly in time, so unlike the tight loop they show how often the server is busy.
- **Server tax:** a second client sends `GetInputFocus` once per millisecond and times each round trip while a
  load thread checks at a given rate.

Reproduce with `spikes/spike-capture/run-matrix.sh <label> <out.tsv> [none|xrender|glx]`.

## 1. Round trip, tight loop (1,000,000 checks each)

| Compositor | Target | Method | p50 | p99 | p99.9 | max | allocs/check |
| --- | --- | --- | --- | --- | --- | --- | --- |
| none | root | x11rb | 16.4 | 32.9 | 1283 | 2116 | **5** |
| none | root | raw | 16.6 | 33.3 | 1243 | 1625 | **0** |
| none | root | shm | 16.0 | 31.4 | 1278 | 1796 | 3 |
| none | window | x11rb | 16.4 | 32.3 | 1265 | 2105 | 5 |
| none | window | raw | 16.5 | 33.0 | 1251 | 1760 | 0 |
| none | window | shm | 16.0 | 32.4 | 1255 | 5468 | 3 |
| picom xrender | root | x11rb | 15.3 | 32.5 | 1277 | 3704 | 5 |
| picom xrender | root | raw | 15.9 | 35.0 | 1274 | 2238 | 0 |
| picom xrender | root | shm | 14.3 | 30.8 | 963 | 18558 | 3 |
| picom xrender | window | x11rb | 15.6 | 31.6 | 1295 | 1991 | 5 |
| picom xrender | window | raw | 16.3 | 33.1 | 1293 | 3611 | 0 |
| picom xrender | window | shm | 15.2 | 30.3 | 1288 | 2149 | 3 |
| picom glx | root | x11rb | 16.0 | 34.5 | 804 | 15087 | 5 |
| picom glx | root | raw | 17.1 | 42.7 | 1302 | 2156 | 0 |
| picom glx | root | shm | 15.6 | 34.7 | 1333 | 2079 | 3 |
| picom glx | window | x11rb | 16.6 | 40.6 | 1327 | 2919 | 5 |
| picom glx | window | raw | 16.8 | 42.3 | 1303 | 2920 | 0 |
| picom glx | window | shm | 15.6 | 37.8 | 1302 | 7391 | 3 |

Throughput was 46,000–56,000 checks/s in every case. About 0.1% of checks took over 1 ms (one per server busy
period, see §4).

- The three methods cost the same. MIT-SHM brings nothing for a 3×3 patch.
- x11rb allocates 5 times per check and SHM-through-x11rb 3 times; the raw path allocates nothing (ADR 0003).
- Root and window reads cost the same, with or without a compositor.

## 2. Paced round trip: what the engine will see

GetImage at a fixed rate through the root window, raw method, 20 s per run (10 s for the A/B rounds).

| Compositor | Rate | p50 | p90 | p99 | max | checks > 4 periods late |
| --- | --- | --- | --- | --- | --- | --- |
| none | 500/s | 38 | 415 | 1179 | 3749 | 0 |
| none | 2,000/s | 20 | 49 | 1134 | 17667 | 1 |
| none (6 more runs) | 2,000/s | 21–30 | — | 1108–1159 | ≤ 1525 | 0–2 |
| picom xrender | 2,000/s | 19–21 | 396–1250 | 2672–2693 | 4142 | 1,213 in 20 s |
| picom glx | 2,000/s | 23–30 | 80–111 | 3098–3164 | 5131 | 1,430 in 20 s |

Window-buffer reads were run the same way under both compositors and matched root reads within noise:
xrender p99 2.69 ms for both; glx p99 3.10–3.16 ms for window and 3.15 ms for root.

For comparison, a trivial request (`GetInputFocus`) paced at 2,000/s without a compositor has p99 0.59–0.64 ms.

## 3. Server tax: another client's round trip

`GetInputFocus` from a separate client, once per millisecond.

| Compositor | Glint-like load | p50 | p90 | p99 | p99.9 | max |
| --- | --- | --- | --- | --- | --- | --- |
| none | none (7 runs) | 8–47 | 50–71 | **582–725** | 1055–1260 | 1112–10188 |
| none | GetImage 500/s (4 runs) | 7–15 | 47–88 | **749–1057** | 1226–1461 | 1609–4626 |
| none | GetImage 2,000/s (7 runs) | 6–19 | 64–152 | **964–1168** | 1148–1350 | 1371–17155 |
| none | GetImage max, ~50,000/s | 15 | 157 | 1135 | 1318 | 2258 |
| none | GetInputFocus 2,000/s (3 runs) | 6 | 9–16 | **588–693** | 780–850 | 807–1219 |
| none | GetInputFocus max, ~200,000/s (3 runs) | 6–9 | 8–23 | 604–881 | 883–970 | 2256–2541 |
| picom xrender | none | 18 | 245 | 3402 | 4016 | 13809 |
| picom xrender | GetImage 2,000/s | 8 | 1272 | 2623 | 2789 | 3576 |
| picom glx | none | 9 | 540 | 1419 | 1786 | 2200 |
| picom glx | GetImage 2,000/s | 8 | 1423 | 1731 | 2540 | 3224 |

- **Balanced adds about +0.45 ms at p99** to other clients (median 1,103 vs 648 µs). Eco adds about +0.25 ms.
  The plan's target was under 0.2 ms (§5.6).
- **The cost is specific to GetImage.** The same number of trivial requests adds nothing. GetImage makes the
  NVIDIA X server wait for GPU work other clients have queued (a readback sync), and every client waits with it.
  Request count is not the lever; GetImage count is (Eco, or the DAMAGE backend in Phase 6).
- A compositor costs other clients far more than Glint does: picom xrender alone puts their p99 at 3.4 ms.

## 4. Server busy periods ("stalls")

`spike-capture stalls`: a tight GetImage loop for 10 s, logging every round trip above 300 µs.

| Compositor | Stalls/s | Length p50 / p99 / max | Pattern |
| --- | --- | --- | --- |
| none | 120 | 1.14 / 1.30 / 1.65 ms | pairs 1.6 ms apart, every ~17.0 ms; not locked to the 60 Hz refresh |
| picom xrender | 241 | — | — |
| picom glx | 182 | — | — |

With no compositor the X server is busy about 10–12% of the time, serving other clients on what looks like
a ~16 ms animation timer plus work. Separately, **every configuration showed occasional 10–18 ms server stalls,
roughly once a minute, including runs with no Glint load at all** (baseline probe max 10.2 ms; 17.2 ms seen by
both clients at once). No client can avoid these.

**Are the big stalls periodic?** (2026-11-13, after Phase 1.) A light probe, GetInputFocus at 1,000/s for 300 s
with no window (`spike-capture stalls --method focus --rate 1000 --secs 300`), saw two stalls above 5 ms: 11.3 ms
at 28 s and 9.6 ms at 185 s. Smaller busy periods: 25 per second, p50 0.6 ms, p99 1.0 ms. The big ones are not on
a timer. The Xorg log (`~/.local/share/xorg/Xorg.0.log`) shows NVIDIA output probes only at server start and once
about 5 hours later, so a program polling the monitors (which makes the driver probe the disconnected HDMI-0) is
not the cause. The cause is still unknown; candidates are other clients' GPU work and the driver.

## 5. Injection (`spike-inject`)

Run inside Xvfb 21.1.26 (`xvfb-run -a -s "-screen 0 1280x720x24"`, display `:99`), because nothing may inject
into the real desktop before Phase 2's exit. Xvfb is a software X server, so these numbers are the protocol path
(two client sockets and the server's event delivery), not the NVIDIA desktop; the real-desktop number comes at
Phase 2's exit. 2,000 trials per event type; time from sending `FakeInput` (with flush) to the event arriving
on the test window's own connection:

| Event | p50 | p90 | p99 | p99.9 | max |
| --- | --- | --- | --- | --- | --- |
| Button press | 6.0 µs | 6.2 µs | 8.0 µs | 64.8 µs | 88.2 µs |
| Button release | 4.3 µs | 4.5 µs | 17.7 µs | 62.4 µs | 75.6 µs |
| Key press | 5.9 µs | 6.1 µs | 8.0 µs | 22.9 µs | 87.4 µs |
| Key release | 5.9 µs | 6.0 µs | 18.5 µs | 23.9 µs | 29.7 µs |

Behavior the engine relies on, all confirmed:

- **Defensive release is harmless.** Releases for buttons 1, 2, 3, 8, 9 and keycodes 8–255 that weren't held
  produced no events.
- **Connection D can release what connection B pressed**, for a button and for a key.
- **`XTestGrabControl(impervious)` is needed.** While another client held a server grab, the impervious injector's
  press arrived at once; a plain one arrived only after the ungrab (it was held back for the whole 300 ms grab).
- **A 0 ms hold works.** A press and a release in one flush arrive as press, then release.
- No stray events after 8,000 timed events.

## 6. UI (`spike-ui`)

- Pulse's `theme.slint` and `components.slint` compile in place through a Slint library path (`@pulse/…`) with
  Pulse's `cosmic-dark` style. Nothing was copied out of Pulse.
- Slint 1.18.1's generated code expands `ItemTreeVTable_static!`, which carries `#[allow(unsafe_code)]`. That is a
  hard error (E0453) under `#![forbid(unsafe_code)]`. Fix: the generated view goes in its own crate under
  `deny(unsafe_code)`, and the hand-written UI crate keeps `forbid` (ADR 0002).
- `BackendSelector` with winit plus the software renderer works. `slint::set_xdg_app_id` must come after the
  backend is selected and before the window is shown. It sets `WM_CLASS` to `"", "glint-spike"`, with an
  **empty instance name**.
- The window manager's close request (`xdotool windowquit`, the same request as the title-bar button) reaches
  `on_close_requested`, which opens the dialog; the window stays open.
- Idle CPU while shown and untouched: 0 ticks in 3 s.
- Screenshots: `spikes/spike-ui/screenshots/` (idle, started, close dialog).
- Pulse's hero button is the reverse of §12.6: **Start is the white primary button and Stop is dark with a white
  outline** (`ButtonKind.danger`). Pulse's source wins (A.7); §12 is reconciled with the kit in Phase 4.

## 7. `glint-core` (Phase 1)

Same machine; `cargo bench -p glint-core` (criterion 0.8.2, release profile with fat LTO; 3 s per benchmark after
1 s of warm-up). The lookup table is Appendix B's rule set: one include color, one exclusion.

| Benchmark | Time (criterion's estimate) | Target |
| --- | --- | --- |
| 3×3 verdict, color present (9 lookups + pixel agreement) | 8.4 ns | under 100 ns (§19) |
| 3×3 verdict, color absent | 8.4 ns | |
| 3×3 verdict, edge of an appearance | 8.5 ns | |
| 3×3 verdict, 9 random colors (table lookups miss the cache) | 15.4 ns | |
| Verdict + trigger step (one whole decision after the capture) | 13.0 ns | under 2 µs (§3) |
| Lookup-table build, 16.7 M colors on 12 threads | 145 ms | 0.1–0.3 s (§5.4) |

- The 4 MiB table fits in this CPU's 12 MB L3, so even a cache-missing check costs about 15 ns. The decision is
  four orders of magnitude below the capture round trip (16–20 µs).
- The same build takes ~145 ms in the dev profile (glint-core is built with `opt-level = 3` there). The engine's
  low-priority pool will use fewer threads, so expect a few hundred milliseconds when a range is edited.
- `cargo test -p glint-core` (dev profile): 79 tests in ~6.5 s, most of it the exhaustive table test (8 rule
  sets × 16,777,216 colors against the reference) and 2,000 property-test cases for the trigger.

## 8. Phase 2: lab scenarios (Xvfb, then the real desktop)

`glint-lab <scenario> --count N` (release build) starts its own Xvfb (1280×720, 24-bit, black root) and its own
`glintd`, paints the color in a 40×40 patch at the monitor's center, and records when the engine's
`ButtonPress` reaches the canvas. Response is timed from just before the drawing request, so it includes the
lab's own request path (about 10 µs) and the event's delivery. Xvfb is a software server with no GPU and no
other clients, so these numbers are the engine's floor, not the NVIDIA desktop's.

| Scenario | Trials | Clicks | Extra | Missed | Response p50 / p95 / p99 / max |
| --- | --- | --- | --- | --- | --- |
| `basic` (on 30–500 ms, off 60–600 ms) | 300 | 300 | 0 | 0 | 0.29 / 0.52 / 0.57 / 0.60 ms |
| `near-miss` (4 colors that must not count) | 200 | 0 | 0 | — | — |
| `arm-while-present` | 20 | 20 | 0 | 0 | 0.27 / 0.36 / 0.36 / 0.36 ms |

- A p50 of ~0.29 ms matches the budget in §3: an average 0.25 ms wait for the next check at 2,000 checks/s, plus
  the round trip and the event's delivery.
- `cargo test` runs the same three scenarios on every commit with 20, 16 and 3 trials.
- The engine's own telemetry during the tests: about 2,000 checks per second, capture p50 18–24 µs, decision
  (capture returned → press written) about 3 µs.
- The 300-trial run found two bugs, both fixed. The engine sent `fired` notices to clients that never subscribed
  and then cut a non-reading client off. The lab scored clicks against the wrong appearance when the engine beat
  the lab's own round trip.

### On the real desktop: the Phase 2 exit run

`glint-lab basic --real-desktop --count 100` (release build), run by the owner on 2026-11-13 in their normal
session (the environment above, no compositor running). The lab's full-screen window on HDMI-1 took the clicks.

| Scenario | Trials | Clicks | Extra | Missed | Response p50 / p95 / p99 / max |
| --- | --- | --- | --- | --- | --- |
| `basic` (on 30–500 ms, off 60–600 ms) | 100 | 100 | 0 | 0 | 0.48 / 0.70 / 0.71 / 0.71 ms |

- 0.1–0.2 ms slower than Xvfb. The difference isn't broken down yet; Phase 3's report adds the per-stage split
  from `fired`. A likely part is GetImage waiting for the GPU to finish the lab's own drawing (ADR 0004 traced
  the server tax to the same kind of wait).
- Well inside p99 ≤ 2 ms and max ≤ 10 ms, but 100 trials say little about p99, and no trial met an X server
  stall. The v1.0 timing verdict is Phase 6's 10,000-trial run (ADR 0004).

## 9. Phase 3: every lab scenario at full count, and CI

`glint-lab all --minutes 2` (release build) on 2026-11-13, each scenario in its own sandbox (Xvfb 1280×720 with its
own `glintd`). Part of the run overlapped a full `cargo nextest` run on the same machine, which is where the X
server stalls in `arm-while-present` and `live-edit` came from; the report lists the trials they touched apart.

| Scenario (count) | Appearances | Clicks | Extra | Missed | Possible misses, reported | Response p50 / p99 / max |
| --- | --- | --- | --- | --- | --- | --- |
| `basic` (100) | 100 | 100 | 0 | 0 | 0 | 0.32 / 0.56 / 0.56 ms |
| `shades` (100, 64 shades) | 100 | 100 | 0 | 0 | 0 | 0.30 / 0.55 / 0.55 ms |
| `near-miss` (100) | 0 | 0 | 0 | — | 0 | — |
| `distractor` (80 distractors, 20 real) | 20 | 20 | 0 | 0 | 0 | 0.29 / 0.54 / 0.54 ms |
| `flicker` (50) | 50 | 50 | 0 | 0 | 0 | 0.30 / 0.61 / 0.61 ms |
| `boundary-noise` (50) | 50 | 50 | 0 | 0 | 0 | 0.23 / 0.59 / 0.59 ms |
| `fade` (30) | 30 | 30 | 0 | 0 | 0 | 0.29 / 0.56 / 0.56 ms |
| `rapid` (10 rounds) | 60 | 58 | 0 | 0 | 2 | 0.29 / 0.56 / 0.56 ms |
| `arm-while-present` (20) | 20 | 16 | 0 | 0 | 4 | 0.37 / 3.85 / 3.85 ms |
| `live-edit` (10 rounds, 50 edits) | 30 | 28 | 0 | 0 | 2 | 0.29 / 0.60 / 0.60 ms |
| `stall` (50, 54 freezes of 30 ms) | 45 | 25 | 0 | 0 | 20 | 0.54 / 15.34 / 15.34 ms |
| `soak` (2 min, 149 random steps) | 241 | 241 | 0 | 0 | 0 | 0.31 / 0.54 / 0.55 ms |

- Every appearance that called for a click got exactly one, or the engine reported a possible miss: 790
  appearances, 0 extra clicks, 0 unexplained misses. `stall` forces possible misses on purpose (ADR 0010), and 11
  of its clicks were late by the 15 ms freeze, as designed.
- The other scenarios:
  - `two-engines`: the second engine exited with code 3.
  - `feedback` (3 rounds): each round tripped the safety stop 151 ms after its first click, after 3 clicks.
  - `action-matrix` (29 cases): one press and one release each. The server saw holds of 0–1, 10–11 and 50 ms.
  - `kill-mid-press`: the button, and Ctrl+a, stayed held after SIGKILL until the restarted engine released them.
  - `action-equals-hotkey`: F7, F8, F6 and the reverse case were rejected.
- The response at p50, by stage: wait 0.18–0.49 ms, capture 11–21 µs, decide about 1 µs, inject about 3 µs,
  delivery 21–33 µs. The wait is half a check period on average, and longer after a freeze.
- `soak`'s engine memory grew 5 MiB in its first steps, then stayed flat: 9.0 → 14.2 MiB in the release build,
  and the same +5 MiB after 8 s and after 2 min in debug builds. That is the allocator keeping one freed 4 MiB color
  table from a profile edit, not a leak.
- **CI** (GitHub Actions, ubuntu-24.04, run 43426780742):
  - `tests and lab scenarios (Xvfb)`: all 170 tests, the 17 scenarios included, in 154 s (4.5 min with the
    build).
  - `lab scenarios under picom`: 17 of 17 in 127 s.
  - `fmt and clippy` and `cargo-deny`: under a minute each.
- The lab found two bugs, both fixed:
  - the trigger timed each check by its reply only (ADR 0011);
  - the raw X connection failed without an Xauthority file, so the first CI run failed every Xvfb test.

## Conclusions for the defaults (Phase 0 exit)

- **Speed preset: Balanced (2,000/s) confirmed.** Paced response is dominated by the server's own busy periods
  (~1.1 ms at p99 without a compositor), not by the pacing interval. Max buys almost nothing; Eco adds up to
  1.5 ms of waiting for a ~0.2 ms smaller server tax. Expected engine-side response without a compositor:
  p50 ≈ 0.27 ms, p99 ≈ 1.6 ms.
- **Backend: `screen` (core GetImage) confirmed**, on the allocation-free raw path (ADR 0003). The auto rule
  stands: SHM only for large patterns (no gain at 3×3), and `window-buffer` under a compositor (same round-trip
  cost as root, and it skips the compositor's frame).
- **Plan targets these numbers contradict** (ADR 0004):
  - Server tax at Balanced is +0.45 ms at p99 on NVIDIA, not under 0.2 ms.
  - With picom, the paced capture p99 is 2.7–3.2 ms, so the engine p99 ≤ 2 ms target is not reachable on
    this machine under a compositor.
  - X server stalls of 10–18 ms about once a minute put the 10 ms hard ceiling outside Glint's control;
    `glint-lab` must report server-side stalls separately.
