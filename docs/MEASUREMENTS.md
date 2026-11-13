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

## 5. Injection (`spike-inject`)

Not measured yet: it injects input, so it may run only inside Xvfb or Xephyr, and neither is installed. The spike
is built and refuses to run on display `:0`.

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
