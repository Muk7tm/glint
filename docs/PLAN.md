# Glint — Low-Latency Color-Triggered Clicker for Linux X11

**Complete build plan · v1.0 · 13 November 2026**

> **Working name:** *Glint* — a sibling to your **Pulse** app. Rename freely; nothing in the design depends on the name.
>
> **Stack:** Rust (2024 edition) · engine `glintd` (headless, pure-Rust X11 via `x11rb`) · UI `glint` (Slint 1.18+) · local Unix-socket IPC · X11 on any distribution.

---

## What Glint does

Glint watches a tiny patch of pixels at the exact center of a monitor, or of a window you pick. The moment that patch turns into a shade of the color you chose, Glint sends exactly one mouse click or key press, then refuses to fire again until the color has clearly left and comes back. It runs as two programs: a small engine that does all of the watching and clicking, and a separate Slint window you use to set it up. The engine never waits on the window, so the UI can be slow, hidden, or closed without touching response time.

The one design idea everything hangs on: **speed lives on the rising edge, safety lives on the falling edge.** The engine fires on the very first check that passes a strict "this is the color" test, with no waiting. All of the protection against double clicks is spent on deciding when the color has *really gone*. Tightening safety therefore never adds latency, and making the engine faster never adds clicks, which is exactly your rule that response time and one-click-per-appearance must never conflict.

**Targets:** engine-side response p99 ≤ 2 ms with a hard ceiling of 10 ms; zero extra clicks and zero missed appearances across the automated test suite in §16.

**Assumptions** (change any of them and the plan still holds):

- *Response time* means from the moment the color exists in the X server's image of the screen (or of the window) to the moment Glint hands the click to the X server. Compositor and game frame delays sit outside the engine, but the lab tool (§16) measures them anyway.
- *"Under 10 ms"* is treated as a hard ceiling; the design aims about an order of magnitude lower.
- A mouse click lands wherever the pointer is, like a normal click. "Click at the watched spot" is an option.
- Pulse is also Rust + Slint, so Glint reuses its theme and components through a shared crate (§12.1). The tokens in §12 were measured from your screenshots pixel by pixel and serve as a cross-check.
- *X11* means an Xorg session on any distribution. Under XWayland, Glint still works for X11 apps only, and says so.

**Out of scope for v1:** Wayland-native capture and input, shape or image recognition, rapid-fire clicking (that's Pulse's job), anti-detection or "humanized" input, and anything that reads or modifies another program's memory. Heads-up: Roblox's rules and many individual games treat automated input as cheating and can ban accounts for it, so keep Glint to places where automation is allowed (your own games and test builds, single-player content, reaction-time tests).

**How to use this document:** §1–§3 explain *why* the design looks the way it does. §4–§12 are the specification. §13–§18 are the project scaffolding. §19 is the build order, with exit criteria for every phase. Appendix A has starter code for the trickiest parts.

---

## Contents

1. [Your rules as an engineering contract](#1-your-rules-as-an-engineering-contract)
2. [Stack decisions](#2-stack-decisions)
3. [Where the milliseconds go](#3-where-the-milliseconds-go)
4. [Architecture](#4-architecture)
5. [Detection pipeline](#5-detection-pipeline)
6. [Input injection](#6-input-injection)
7. [Every way an extra click can happen and what stops it](#7-every-way-an-extra-click-can-happen-and-what-stops-it)
8. [Windows, monitors and the desktop environment](#8-windows-monitors-and-the-desktop-environment)
9. [Hotkeys](#9-hotkeys)
10. [IPC protocol](#10-ipc-protocol)
11. [Settings and profiles](#11-settings-and-profiles)
12. [UI plan matched to Pulse](#12-ui-plan-matched-to-pulse)
13. [Repository layout](#13-repository-layout)
14. [Dependencies and Cargo workspace](#14-dependencies-and-cargo-workspace)
15. [Development environment](#15-development-environment)
16. [Testing and verification](#16-testing-and-verification)
17. [Diagnostics and observability](#17-diagnostics-and-observability)
18. [Packaging and distribution](#18-packaging-and-distribution)
19. [Milestones](#19-milestones)
20. [Risks and mitigations](#20-risks-and-mitigations)
21. [Decisions worth revisiting](#21-decisions-worth-revisiting)
- [Appendix A. Code skeletons](#appendix-a-code-skeletons)
- [Appendix B. Example profile](#appendix-b-example-profile)
- [Appendix C. X11 debugging toolkit](#appendix-c-x11-debugging-toolkit)
- [Appendix D. References](#appendix-d-references)

---

## 1. Your rules as an engineering contract

Every rule you gave becomes a precise requirement, a mechanism that enforces it, and an automated test that proves it. If a test in the right-hand column ever fails, the build is broken.

| # | Your rule | Precise meaning in Glint | Mechanism | Proven by (§16) |
|---|---|---|---|---|
| R1 | One click only per appearance | Exactly one press and one release per appearance (defined below) | Edge-triggered latch (§5.5), latch set before any other decision; one engine per display (§4.4) | `basic`, `shades`, `soak` |
| R2 | No extra clicks from accidents or color mismatches | Colors outside the range never fire; noise inside an appearance never re-fires | Strict enter range + pixel agreement (§5.3) + exclude colors and hysteresis (§5.4) | `near-miss`, `flicker`, `boundary-noise` |
| R3 | Response under 10 ms | p99 ≤ 2 ms engine-side; never above 10 ms | 2,000 checks/s, one X round trip per check, click sent inline, optional real-time priority (§5.6) | `basic` latency report |
| R4 | Speed and one-click-per-appearance never conflict | Faster settings can't add clicks; safer settings can't add delay | Rising-edge / falling-edge split (§5.5) | `flicker`, `rapid` |
| R5 | Similar shades only, never other colors even if similar | The range is shades of one hue, tightly bounded; explicit exclusions always win | OKLCh shade model compiled into a lookup table (§5.4) | `near-miss`, `distractor` |
| R6 | Normal mouse clicks or specific keyboard keys | Any mouse button or any physical key, with a hold time | XTest injector (§6) | `action-matrix` |
| R7 | Nothing while the color is gone; fire only at the start | Firing requires having *seen* the color absent first. No click when you start watching, when the window moves, or when it comes back to front while the color is already showing | Observed-absence re-arm + every (re)start begins latched (§5.5, ADR 0007) | `arm-while-present`, `move`, `focus` |

> **Definition — appearance.** An appearance begins at the first check whose result is *Enter* after Glint has observed the color absent for at least `rearm_absent_ms` **and** at least `rearm_absent_samples` checks (observed: a stall between two checks doesn't count as watching, §5.5). It ends when that same absence condition is met again. Anything that vanishes for less than the re-arm window is part of the same appearance.

---

## 2. Stack decisions

### Rust — yes, for the right reasons

Rust is in the same speed class as C and C++, not faster than them. What makes it the right pick here is predictability: there are no garbage-collector pauses to blow up the worst-case response, threads can't race on shared data by accident, and `x11rb` (a pure-Rust X11 client) lets the engine ship as one static binary that runs on any distribution with no library dependencies at all. The real latency wins in this project are architectural (how often the screen is checked, how many round trips each check costs, how the hot thread is scheduled), and §3 shows where every microsecond goes.

### Slint — yes

Pulse already proves it: it's built with Rust and Slint and has exactly the look you want, so Glint can share its theme and components instead of re-creating them (§12.1). Beyond that, Slint fits well. Its declarative `.slint` files compile to native code, it has a small footprint, its software renderer keeps the UI completely off the GPU your game is using, and its live preview makes matching Pulse's look fast. Since 1.17 it also has built-in tooltips and a system-tray icon element; the current stable line is 1.18.x. Because the UI is a separate process, its performance can't affect clicking anyway, so the choice comes down to design control and developer experience, and Slint wins there.

**License: GPLv3.** Glint is open source, so it uses Slint under GPLv3, which needs no on-screen credit, so no About screen is required. Under Slint's GPLv3 option the distributed `glint` UI binary must be GPLv3; to keep things simple this plan licenses the whole project GPL-3.0-or-later. The one exception is the shared Slint kit (§12.1), which stays MIT OR Apache-2.0 so Pulse can keep using it whatever Pulse's own license is (§21).

| Alternative considered | Verdict | Why |
|---|---|---|
| egui | No | Immediate-mode redraws; reproducing a custom design pixel-for-pixel is harder |
| iced | Maybe | Solid, but the styling and widget set need more custom work than Slint |
| GTK4 (gtk-rs) | No | Heavier; its theming fights a custom look like Pulse's |
| Tauri / web UI | No | A browser engine for a 400 px panel is overkill |
| C/C++ engine | No | Same machine code, more risk; Rust gives the speed with safety |

### X11 access — `x11rb` with its pure-Rust connection

`x11rb` covers everything Glint needs: core `GetImage`, MIT-SHM, XTest, DAMAGE, Composite, RandR, XInput 2, XFixes, and the screensaver extension, each behind a cargo feature. Its `RustConnection` speaks the X protocol itself instead of linking `libxcb`, which is what makes a fully static engine possible (§18).

### Two processes, not two threads

The engine (`glintd`) and the UI (`glint`) are separate processes with separate X connections. A UI crash can't interrupt a click mid-press, an engine crash can't freeze the UI, the UI's rendering never competes with the hot loop for locks or caches, and the engine can keep running after you close the window, exactly like Pulse's "Keep running".

---

## 3. Where the milliseconds go

```
 the app draws the color
        │  compositor, if one is running: up to 1 refresh before the screen image updates   ← outside Glint
        ▼
 X server's screen image / window buffer now contains the color ............ t0 "appeared"
        │  wait for Glint's next check ............. 0 – 0.5 ms   (Balanced = 2,000 checks/s)
        ▼
 GetImage round trip for a 3×3 patch ............... 0.016 – 0.02 ms; up to ~1.3 ms while the server is busy (Phase 0)
        ▼
 classify 9 pixels + trigger decision .............. < 0.002 ms
        ▼
 XTest press sent and flushed ...................... ~0.01 – 0.03 ms ...... t1 "clicked"
        ▼
 X server delivers the ButtonPress / KeyPress ...... ~0.02 – 0.2 ms
        ▼
 the app reads the event ........................... app-dependent (often its next frame)  ← outside Glint
```

The only part that scales with a setting is the wait for the next check:

| Speed preset | Checks per second | Worst-case wait | Average wait | Cost |
|---|---|---|---|---|
| Eco | 500 | 2.0 ms | 1.0 ms | lowest |
| **Balanced (default)** | **2,000** | **0.5 ms** | **0.25 ms** | low |
| Max | as fast as round trips allow (often 5,000–20,000) | ≈ one round trip | ≈ half a round trip | one CPU core busy, X server busier |

Even Eco is five times under your 10 ms rule. Max mostly buys fractions of a millisecond and costs a CPU core, which is why Balanced is the default.

The number that actually dominates on most desktops is the **compositor**. When one is running, the screen image updates once per refresh, up to 16.7 ms after the app drew its frame at 60 Hz (6.9 ms at 144 Hz). Glint can't speed up a compositor, but in window mode it can read the window's *own* buffer before the compositor paints it (the window-buffer backend, §5.2), which removes that delay entirely. In screen mode, turning the compositor off or letting it unredirect fullscreen windows gets the same benefit.

**Engine-side targets** (Phase 0 replaces every "~" above with numbers measured on your machine):

| Measure | Target | Hard limit |
|---|---|---|
| p50 engine response | ≤ 0.5 ms | — |
| p99 engine response | ≤ 2 ms | — |
| Max over 10,000 appearances | ≤ 5 ms | 10 ms |
| Extra clicks | 0 | 0 |
| Missed appearances lasting at least one frame | 0 | 0 |

**Measured in Phase 0** on the reference machine (NVIDIA 620.70.06, Xorg 21.1.26; `docs/MEASUREMENTS.md`, ADR 0004):

- **Round trip:** p50 is 16–20 µs. The tail comes from the X server being busy with other clients (about 10% of
  the time with no compositor): paced at 2,000/s, p99 is 1.1 ms.
- **Expected engine response without a compositor:** p50 ≈ 0.27 ms, p99 ≈ 1.6 ms, which meets the targets.
- **With picom:** the paced capture p99 rises to 2.7–3.2 ms because of server-side compositing and GPU syncs, so
  p99 ≤ 2 ms is not reachable there.
- **Server stalls:** the X server stalls for 10–18 ms about once a minute whatever the load, so a check that
  lands in one exceeds the 10 ms ceiling regardless of Glint. How v1.0 counts those is open (ADR 0004).

There is no "a pixel changed" event in X11, which is why the default design polls a tiny region rather than waiting for notifications. Polling a 3×3 patch is cheap and deterministic. Event-driven checking via the DAMAGE extension is added later as an optimization with self-checks, because not every driver reports every update (§5.2).

---

## 4. Architecture

### 4.1 Processes and connections

```
┌──────────────────── glint (UI, Slint) ────────────────────┐   ┌──────── glint-ctl (CLI) ────────┐
│ main thread: Slint event loop (never blocks)              │   │ status · start · stop · panic … │
│ io thread:   socket reader/writer ─┐                      │   └────────────────┬────────────────┘
└────────────────────────────────────┼──────────────────────┘                    │
          newline-delimited JSON over a Unix socket: $XDG_RUNTIME_DIR/glint/glintd-<display>.sock
┌────────────────────────────────────┼─────────────────────────────────────────────┼──────────────┐
│ glintd (engine)                    ▼                                             ▼              │
│  controller thread ─ IPC server · settings store · LUT compiler · window tracker · hotkeys      │
│        │  ArcSwap<CompiledProfile>       AtomicU64 control word          ▲ rtrb event ring    │
│        ▼                                                                 │                     │
│  hot thread ─ check ─► classify ─► decide ─► click ─► pace ──────────────┘                     │
│        X conn A: capture       X conn B: input                                                 │
│  telemetry thread ─ turns the event ring into stats for the controller                         │
│  X conn C: controller events (windows, focus, hotkeys, picking) · X conn D: emergency release  │
└─────────────────────────────────────────────────────────────────────────────────────────────────┘
             │ GetImage / DAMAGE                                  │ XTest FakeInput
       ┌─────▼────────────────────── X server (Xorg / XWayland) ───▼────────┐
       │ screen image · window buffers · routes input to the focused app    │
       └────────────────────────────────────────────────────────────────────┘
```

### 4.2 Engine threads and shared state

| Thread | Owns | Must never |
|---|---|---|
| **hot** | X connections A (capture) and B (input) over the allocation-free raw protocol path (ADR 0003), the trigger state machine, the injector | Take a lock, allocate, log, or do any I/O other than X socket traffic and sleeping |
| **controller** | IPC server, settings and profiles on disk, LUT compilation (on a low-priority rayon pool), X connection C (window tracking, focus, hotkeys, picking), RandR monitor tracking | Touch connections A, B or D |
| **telemetry** | Drains the event ring, keeps latency histograms, hands snapshots to the controller | Block the hot thread (it can't: the ring is wait-free) |
| **panic/signal path** | X connection D, opened at startup and kept idle | Be used for anything except emergency releases |

The hot thread talks to the rest of the engine through three lock-free channels:

- **Control word** (`AtomicU64`): `[armed:1][reserved:15][epoch:16][x:16][y:16]`. X11 coordinates are 16-bit, so the whole sample origin fits. The controller bumps `epoch` whenever watching must restart cleanly: start, target moved or resized, focus regained, profile swapped. The hot thread compares `epoch` once per iteration and re-initializes (§5.5).
- **Compiled profile** (`ArcSwap<CompiledProfile>`): the color lookup table, sample pattern, trigger settings, action and pacing period. Reloaded only when `epoch` changes.
- **Event ring** (`rtrb` single-producer/single-consumer): fixed-size records `{kind, t_ns, a, b}` for fires, suppressions, stalls and errors. When full, the hot thread drops the record and increments a counter; it never waits. A wait-free **triple buffer** carries the latest pixels and verdict for the UI's live swatch.

### 4.3 Lifecycle

**Engine startup (`glintd`)**

1. Parse arguments; refuse to start without `DISPLAY`.
2. Take the single-instance lock `$XDG_RUNTIME_DIR/glint/glintd-<display>.lock` with a non-blocking `flock`. If it's taken, exit with code 3 ("already running"); the UI then connects to the running engine.
3. Open X connections A–D and probe extensions and versions into a capability set. Missing XTest is fatal with a clear message; everything else degrades gracefully.
4. **Defensive release:** send XTest releases for mouse buttons 1, 2, 3, 8, 9 and the active profile's key. A release for something not held is a harmless no-op, and this un-sticks anything a killed previous engine left pressed.
5. Load settings and the active profile (migrating older schemas), and compile the color lookup table.
6. Create the socket directory (mode 0700) and socket; start the controller, telemetry and hot threads; request real-time priority for the hot thread (§5.6).
7. Register hotkeys and start window tracking.
8. Report ready, in the **Stopped** state. The engine never starts watching on its own; it waits for Start, a hotkey, or the explicit `--start-on-launch` flag.

**UI startup (`glint`)**

1. Connect to the socket. If no engine answers, spawn `glintd` from the same directory as `glint` (falling back to `$PATH`), fully detached in its own session (`setsid`, stdio to the log file), then retry the connection for up to 2 s.
2. Handshake with a protocol version check. On mismatch (you updated Glint while an old engine kept running) offer **Restart engine**.
3. Subscribe to state, telemetry, and the live swatch, but only while the window is visible.

The engine is detached on purpose: when you choose "Keep running", it must not die with the UI's process group or terminal.

**Closing — same behavior as Pulse's dialog**

| What happens | Engine behavior |
|---|---|
| Window close button, policy **Ask** (default) | UI shows "Close Glint?" with **Cancel / Keep running / Quit Glint** |
| **Keep running** | UI sends `detach` and exits; engine keeps running with hotkeys active; reopening Glint from the apps menu reconnects |
| **Quit Glint** (dialog or footer button) | UI sends `quit`; engine stops, releases input, removes its socket; UI exits |
| Policy **Always keep running** / **Always quit** | No dialog; same as the matching button |
| UI crashes or is killed (socket closes without `detach`) | Engine keeps running only if the policy is *Always keep running*; otherwise it stops (safe default) |
| Engine crashes | UI shows "Engine stopped unexpectedly" with **Restart**; the restart runs the defensive release |
| `SIGTERM`, `SIGINT`, `SIGHUP` to the engine | Graceful stop: release input, clean up, exit 0 |
| Panic inside the engine | Panic hook releases input on connection D, then aborts |
| `SIGKILL` in the middle of a click | Can't be caught; the next engine start's defensive release fixes it, and `glint-ctl release-all` exists |

### 4.4 One engine per display

Two engines watching the same screen would double every click, so the single-instance lock is a correctness feature, not a nicety. The lock and socket are per display (`:0`, `:1`…), and `glint-ctl status` shows the running engine's PID.

---

## 5. Detection pipeline

### 5.1 Where Glint looks

**Screen center.** Monitors come from RandR 1.5 `GetMonitors` (fallbacks: RandR CRTCs, then Xinerama, then the whole root window). You pick a monitor; the default is the primary. "Center of the screen" is the center of that monitor, never of the whole root window, which on a two-monitor setup sits on the seam between them. The center is `(x + w/2, y + h/2)`. With an even width or height the true center falls *between* pixels, so patterns anchor on the 2×2 block around it (§5.3): the anchor pixel is `(x + ⌊w/2⌋, y + ⌊h/2⌋)`, which is the center pixel for an odd size and the pixel just right of and below the true center for an even one; Center 2×2 covers the four pixels around the center, and odd-sized patterns center on the anchor (ADR 0006). An X/Y offset in pixels allows fine-tuning. Monitor changes arrive as RandR notifications and bump the epoch, so a resolution change can't cause a click (R7).

**Window center.** The target is the application's *client* window (the one carrying `WM_STATE`), not the window manager's frame, so the center is the center of the content and excludes the title bar. Its root-relative position comes from `TranslateCoordinates` and is kept current by the window tracker (§8). The window-buffer backend uses window-relative coordinates instead.

**Clipping.** The sample rectangle is clamped inside the monitor or window; if it can't fit (a tiny window), Glint stops with "Window too small".

### 5.2 Capture backends

| Backend | How it reads | Strengths | Weaknesses | Used when |
|---|---|---|---|---|
| `screen` | Core `GetImage` on the root window, Z-pixmap format, tiny rectangle | Universal, simple, works on any local X server | One round trip per check; sees the composited screen | Default for screen mode |
| `screen-shm` | MIT-SHM `GetImage` into a shared `memfd` segment | No pixel bytes through the socket, zero allocations | Same round trip; only pays off for large patterns; local servers only | Auto when the pattern exceeds 32×32, or when Phase 0 shows it's faster |
| `window-buffer` | `GetImage` on the target window itself | Reads the window's pixels *before* the compositor paints them (removes up to one refresh of delay); works when the window is partly covered | Fails while the window is unmapped; 32-bit-deep windows need alpha handling; some apps render into a child window | Auto in window mode when a compositor is running and the self-check passes |
| `damage` (Phase 6) | DAMAGE notifications trigger an immediate check; slow safety checks in between | Near-zero CPU when nothing changes; checks right after each update | Some driver and page-flip paths don't report damage | Opt-in, with automatic fallback to polling |

**Automatic choice:**

```
if mode == window && compositor_running && window_buffer_selfcheck_ok  → window-buffer
else if pattern_area > 1024 && shm_available                           → screen-shm
else                                                                     → screen
```

**Window-buffer self-check.** When a window is picked, Glint samples its center both ways 20 times over 200 ms. If the window-buffer reads are blank (all black or fully transparent) while the screen reads aren't, or the two disagree on more than 10% of samples while the window is in front, Glint falls back to `screen` and says why in the UI. This catches apps (including some Wine games and container-based clients) that draw into a child window the top-level buffer doesn't contain.

**DAMAGE backend details.** Create a damage object on the root (screen mode) or the target window (window-buffer mode) at the bounding-box report level. The hot thread waits on the X socket with `poll()`, using the safety period (default 10 ms) as the timeout. When a `DamageNotify` area intersects the sample rectangle, it subtracts the damage and checks immediately, plus once more about 1 ms later to catch late writes. On every timeout it runs a safety check. If a safety check finds a change no damage event announced, that's a miss; three misses within a minute switch the engine back to polling with the warning "This system doesn't report screen updates reliably; using regular checks."

**Pixel formats.** Never assume BGRX. Read the drawable's depth and visual (red/green/blue masks), the bits per pixel and scanline padding from the pixmap formats in the connection setup, and the image byte order. Precompute a shift and width for each channel and convert every pixel to `0xRRGGBB`:

| Depth | Typical layout | Handling |
|---|---|---|
| 24 / 32 | 8 bits per channel in 32-bit pixels | direct |
| 30 | 10 bits per channel | keep the top 8 bits |
| 16 | RGB 5-6-5 | expand to 8 bits with bit replication |

Row stride is `round_up(width × bpp, scanline_pad) / 8`. A 32-bit-deep (ARGB) target window may contain premultiplied alpha where it is transparent; Glint ignores alpha and warns if the sample point is transparent.

**Errors.** A `BadMatch` or `BadDrawable` during capture (the window was unmapped or destroyed) makes the hot thread release any held input, stop itself, and report "Window hidden" or "Window closed". The controller restores watching only on your next Start.

**Never captured:** the mouse cursor. A hardware cursor isn't part of the screen image, and the X server removes a software cursor before answering `GetImage`.

### 5.3 Sample patterns and pixel agreement

Every pixel in a pattern comes from the same single `GetImage`, so a bigger pattern costs no extra latency. Each check produces one of three results:

- **Enter** — at least `k_enter` pixels are inside the *tight* range.
- **Present** — at least `k_stay` pixels are inside the *wide* range (§5.4).
- **Absent** — neither.

Validation enforces `k_stay ≤ k_enter ≤ n`, and the tight range is always inside the wide range, so Enter always implies Present.

| Pattern | Pixels | `k_enter` | `k_stay` | Use when |
|---|---|---|---|---|
| Pixel | 1 | 1 | 1 | The color covers only the exact center |
| Center 2×2 | 4 | 3 | 2 | Even-sized screens or windows where you want the true center |
| **Square 3×3 (default)** | 9 | 5 | 3 | General use; ignores single-pixel noise and edges |
| Square 5×5 | 25 | 13 | 7 | Large color areas or noisy content |
| Plus | 5 (center + 4 at distance d = `plus_distance`, 1–31, default 2) | 4 | 2 | Thin shapes crossing the center |
| Custom | `offsets = [[dx, dy], …]`: up to 64, within ±31 px | yours (default: a majority) | yours | Anything else |

Pixel agreement is Glint's main noise filter, and it adds zero delay: a single odd pixel (anti-aliasing, a dithered gradient, a one-pixel overlay) can't make or break an appearance. The UI shows one "Must match" control; `k_stay` is derived as `⌈k_enter / 2⌉`, which reproduces every row of the table (the earlier `⌈0.6 × k_enter⌉` gave 8 for 5×5 and 3 for Plus; ADR 0006), and can be overridden in the profile file. A pattern captures its bounding rectangle, so a plus pattern reads (2d + 1)² pixels and counts 5 of them.

### 5.4 Color matching

**Why not RGB distance.** A shade of a color keeps its hue and varies in lightness, and a little in colorfulness. Plain RGB distance is bad at exactly this: a darker shade of your green can sit farther from it in RGB than a yellow-green you never want to match. So Glint matches in **OKLCh**, the cylindrical form of OKLab, a perceptual color space where equal numeric steps look like equal visual steps. It has three axes: **L** (lightness, 0–1), **C** (chroma, i.e. colorfulness), and **h** (hue angle in degrees). A range is a box around your color: `L ± ΔL`, `C ± ΔC`, `h ± Δh`. Lightness can be generous (that's what "shades" means) while hue stays tight (that's what keeps neighboring colors out).

| Model | Range shape | Use when |
|---|---|---|
| **OKLCh (default)** | L ± ΔL, C ± ΔC, h ± Δh | Shades of one hue, with the best separation from neighboring colors |
| HSV | H ± ΔH, S ± ΔS, V ± ΔV (`tol_h`, `tol_s`, `tol_v`) | You think in color-picker terms |
| RGB box | Each channel ± t (`tol_rgb`, 0–255; 0 = exact) | Exact-color matching, or compatibility with other tools |

**Shade presets** (starting values; Phase 1 tunes them against recorded palettes):

| Preset | ΔL | ΔC | Δh | The UI shows |
|---|---|---|---|---|
| Tight | 0.05 | 0.03 | 4° | Lightness ± 5 · Colorfulness ± 3 · Hue ± 4° |
| **Normal (default)** | 0.10 | 0.05 | 7° | Lightness ± 10 · Colorfulness ± 5 · Hue ± 7° |
| Loose | 0.16 | 0.08 | 12° | Lightness ± 16 · Colorfulness ± 8 · Hue ± 12° |

The UI shows ΔL and ΔC multiplied by 100 so they fit Pulse's whole-number steppers. HSV and RGB have their own presets (ADR 0005): ΔH · ΔS · ΔV of 4° · 0.08 · 0.08, 7° · 0.15 · 0.15 and 12° · 0.25 · 0.25, and t of 6, 12 and 20. A color entry keeps the tolerance fields of every model and only the profile's model reads them, so switching models loses nothing; a missing field takes the Normal value for an include color and the Tight value for an exclude color.

**Several include colors.** A profile can hold several include colors (the same object under different lighting, for example). A pixel matches if it's inside any include range and inside no exclude range.

**Exclude colors** implement your rule "never detect any other colors even if they're similar". Add the near-miss colors you never want, picked from the screen or recorded, each with its own small range. Exclusion always wins, and the UI warns when an exclude range cuts into an include range.

**Hysteresis.** Every include color has two ranges: a **tight** range (the tolerances above) that is needed to *start* an appearance, and a **wide** range (tolerances × `exit_margin`, default 1.5) that is enough to count as *still there*. A color wobbling right at the edge (gradients, anti-aliasing, lighting changes, video noise) therefore can't flicker in and out and re-fire. Exclusions apply to both ranges identically, so the tight range always stays inside the wide one.

**Achromatic guard.** When your color has very low chroma (C < 0.02: grays, near-white, near-black), hue is meaningless and is ignored; only L and C are compared. The UI warns that grays are hard to tell apart from backgrounds. The guard also works the other way (ADR 0005): when your color *is* colored, a pixel with chroma below 0.02 fails the hue test, because a gray pixel's hue is noise (`#818080` has h 17°, `#808180` h 146°), so a muted target can't match gray noise in a speckled pattern. HSV uses the same rule with chroma `(max − min) / 255`, which, unlike saturation, stays near 0 for dark noise.

**Auto range from samples.** Collect 3–10 real occurrences with the *Sample center* hotkey or *Record 5 s* (§12.9). Glint computes the min and max of L, C and h (circularly, so 355° and 5° are 10° apart), adds a margin of 20% of the spread with floors of ΔL 0.02, ΔC 0.015 and Δh 3°, and proposes the result for you to accept.

**Compiled lookup table.** Every rule set compiles into a table holding 2 bits for each of the 16,777,216 possible 24-bit colors (4 MiB): `0` absent, `1` wide range only, `2` tight range, `3` excluded. The hot loop does one table lookup per pixel: no floating point, no model-specific branches, and bit-identical results in the engine, the UI preview, and the tests. The table is built in parallel on the controller's low-priority thread pool (expect 0.1–0.3 s), prefaulted, and published with an atomic pointer swap. The hot loop keeps using the old table until the new one is live and then restarts watching through the epoch, so editing a range can never cause a click (R7). An exhaustive test compares all 16.7 million entries against the reference math.

**OKLab reference math** (Björn Ottosson's definition):

```text
lin(c) = c/255 ≤ 0.04045 ? (c/255)/12.92 : ((c/255 + 0.055)/1.055)^2.4      // 256-entry table
l = 0.4122214708·r + 0.5363325363·g + 0.0514459929·b
m = 0.2119034982·r + 0.6806995451·g + 0.1073969566·b
s = 0.0883024619·r + 0.2817188376·g + 0.6299787005·b
l' = ∛l     m' = ∛m     s' = ∛s
L = 0.2104542553·l' + 0.7936177850·m' − 0.0040720468·s'
a = 1.9779984951·l' − 2.4285922050·m' + 0.4505937099·s'
b = 0.0259040371·l' + 0.7827717662·m' − 0.8086757660·s'
C = √(a² + b²)          h = atan2(b, a) in degrees, wrapped to [0, 360)
hue_distance(h1, h2) = min(|h1 − h2|, 360 − |h1 − h2|)
```

Test vectors (cross-check them against the `palette` crate in a unit test): `#FFFFFF → L 1.0000, a 0, b 0` · `#FF0000 → L 0.6280, a 0.2249, b 0.1258` · `#0000FF → L 0.4520, a −0.0325, b −0.3115`.

**What Glint reads versus what you see.** Night-light tools such as redshift change the monitor's gamma ramps, not the pixels Glint reads, so detection is unaffected, but what you see on screen can differ from what Glint reads. Always pick colors with Glint's own picker, never by eye or from a screenshot taken elsewhere. Compositor effects (window transparency, dimming inactive windows, shaders, fade animations) *do* change the pixels in screen mode; the window-buffer backend reads the window before those effects are applied.

### 5.5 The trigger state machine

States: **Stopped** · **Paused** (started, but the target window isn't in front) · **Idle** (watching; absence has been observed) · **Confirming** (only if you enable confirmation) · **Latched** (fired or (re)started; waiting for an observed absence).

| From | Check result | Condition | To | Output |
|---|---|---|---|---|
| Stopped | — | Manual Start (button, hotkey, `start`, `--start-on-launch`) | **Latched** with a fresh absence count, whatever the color is doing; **Idle** with `fire_if_present_on_start` | — |
| Idle | Enter | `confirm_samples = 1` and `confirm_min_ms = 0` | Latched | **FIRE** |
| Idle | Enter | confirmation enabled | Confirming | — |
| Idle | Present or Absent | — | Idle | — |
| Confirming | Enter | count and time reached | Latched | **FIRE** |
| Confirming | Present or Absent | — | Idle | — |
| Latched | Enter or Present | — | Latched (absence counter reset) | — |
| Latched | Absent | absent for ≥ `rearm_absent_ms` **and** ≥ `rearm_absent_samples` checks, both counted from the *first observed* absent check; between two absent checks at most the check gap counts (four periods, ≥ 2 ms) | Idle | — |
| Any watching state or Paused | — | Epoch changed (moved, resized, profile edited, back in front) | **Latched** with a fresh absence count; `fire_if_present_on_start` doesn't apply | Release held input |
| Any watching state | — | Target window left the front (*Only click in this window*) | Paused | Release held input |
| Paused | any | — | Paused | — |
| Any | — | Stop, emergency stop, safety stop, target hidden or closed, screen locked | Stopped; only a manual Start leaves it (a restart doesn't) | Release held input |

**What FIRE does, in this order.** First it sets the latch, so whatever happens next this appearance is consumed and can never produce a late click. Then two safety checks run. If the previous click was less than the minimum gap ago (`min_fire_interval_ms`, raised to `hold_ms` + 1 so a click can never come due while the previous one is held; ADR 0007), the click is *suppressed* (counted and shown in diagnostics). If this click would exceed `max_clicks_per_sec` within the last second, the **safety stop** trips: Glint stops, releases input, and shows "Stopped: 16 clicks in 1 s — looks like a loop". That catches feedback loops where the click itself makes the color blink. Otherwise the press goes out.

**Why absence is counted from observed checks only.** If the engine stalls for 30 ms (a system hiccup) and the next check finds the color gone, the color may only have blinked for 1 ms. Counting from the last time it was *present* would re-arm on an absence nobody actually saw, and could double-click. Counting only observed absence is never wrong in the dangerous direction. The same goes for gaps *inside* an absence: if a stall hides the color between two short dropouts, two absent checks 45 ms apart don't prove 45 ms of absence. So between two consecutive absent checks Glint credits at most the check gap (four check periods, the stall threshold of §5.6, and at least 2 ms) toward `rearm_absent_ms`; a stall can delay re-arming, never cause it (ADR 0007).

**Why confirmation is off by default.** Glint checks far faster than any screen refreshes, so two consecutive checks usually see the same frame; "confirm over 2 checks" adds delay without proving anything. Pixel agreement (§5.3) filters noise at zero cost. If you do want time-based confirmation, set `confirm_min_ms` to at least one frame (7 ms at 144 Hz) and accept that delay.

**Invariants** (property-tested in §16):

1. Every FIRE happens on an Enter result.
2. Between any two FIREs there is an observed absence run that satisfies both re-arm conditions.
3. No two FIREs are closer together than `min_fire_interval_ms`.
4. After any (re)start, no FIRE happens until an observed absence that satisfies both re-arm conditions, whether or not the color was present (unless `fire_if_present_on_start` on a manual Start; ADR 0007).
5. After a safety stop, no FIRE happens until a manual Start (restarts and focus changes don't count).
6. Each FIRE produces exactly one press and exactly one release, in that order.

| Setting | Default | Range | Meaning |
|---|---|---|---|
| `confirm_samples` | 1 | 1–10 | Matching checks needed before firing |
| `confirm_min_ms` | 0 | 0–50 | Minimum match duration before firing |
| `rearm_absent_ms` | 40 | 0–2000 | Gaps shorter than this belong to the same appearance |
| `rearm_absent_samples` | 2 | 1–100 | Absent checks needed on top of the time |
| `min_fire_interval_ms` | 25 | 0–1000 | Absolute floor between two clicks; the engine uses at least `hold_ms` + 1 |
| `max_clicks_per_sec` | 15 | 1–100 | Safety-stop threshold |
| `fire_if_present_on_start` | off | — | Click immediately if the color is already showing when you press Start |

The implementation is `glint-core/src/trigger.rs`, with scenario tests and property tests for the six invariants; Appendix A.1 summarizes it.

### 5.6 Pacing and scheduling

- **Absolute deadlines.** `next = previous + period`, never `now + period`, so timing errors don't accumulate. In Balanced the hot thread sleeps with `clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME)` until about 50 µs before the deadline and spins the rest. Max doesn't sleep at all.
- **Timer slack.** `prctl(PR_SET_TIMERSLACK, 1)` on the hot thread; by default Linux lets sleeps overshoot by 50 µs.
- **Stall detection.** If a check starts more than four periods late, the engine records a stall (visible in diagnostics) and resynchronizes `next = now`. The observed-absence rule in §5.5 keeps stalls from causing extra clicks.
- **Real-time priority** (`realtime = off | auto | required`). Under a CPU-heavy game, a normal-priority thread can wait 1–5 ms just to be scheduled, which alone could eat half of your 10 ms budget; real-time scheduling cuts that to tens of microseconds. `auto` asks **rtkit** over D-Bus (`MakeThreadRealtime`, SCHED_RR, unprivileged on most desktops; rtkit requires `RLIMIT_RTTIME` to be set first) or uses `RLIMIT_RTPRIO` if you configured `/etc/security/limits.d`. Otherwise it falls back to normal priority and says so in the UI ("Real-time: off — rtkit not available").
- **Never combine real-time priority with Max.** A real-time thread that never blocks can starve its CPU core, and rtkit's `RLIMIT_RTTIME` kills such threads with `SIGXCPU`. Balanced blocks on every round trip and every sleep, so it's safe; selecting Max automatically drops the hot thread to normal priority.
- **CPU affinity.** Optional pinning (`cpu = none | auto | <core>`); `auto` means don't pin unless Phase 0 measurements show a benefit.
- **Memory.** Preallocate everything at start, touch every page of the lookup table once (prefault), and call `mlockall(MCL_CURRENT | MCL_FUTURE)` best-effort; it fails gracefully if `RLIMIT_MEMLOCK` is low.
- **Nothing slow inside the loop.** No logging, locking or allocation in the steady state. x11rb allocates on every request and reply (measured: 5 allocations per GetImage, 3 per ShmGetImage) and locks internal mutexes, so connections A and B use a small raw protocol client with fixed buffers instead, at the same round-trip cost (ADR 0003). Debug builds wrap the loop body in `assert_no_alloc` to enforce this.
- **The X server tax.** The X server handles requests on a single thread, so each check costs it a little time, and with glamor-accelerated drivers a GPU readback sync. The speed presets let you trade speed for system load. Target: Balanced adds under 0.2 ms at p99 to other clients.
  - *Measured in Phase 0 on NVIDIA 620.70.06:* Balanced adds about +0.45 ms and Eco about +0.25 ms at p99.
  - The same number of trivial requests adds nothing. GetImage makes the server wait for GPU work queued by other clients, so the number of GetImages is the lever (Eco, or the DAMAGE backend), not the number of requests (ADR 0004).

---

## 6. Input injection

Glint injects input with **XTest** (`FakeInput`, extension version 2.2), which needs no special permissions on X11. A click is a `ButtonPress` (event type 4) followed later by a `ButtonRelease` (type 5), with `detail` set to the button number. A key is a `KeyPress` (2) and a `KeyRelease` (3) with a keycode. Every request is flushed immediately.

| Setting | Values | Notes |
|---|---|---|
| Mouse button | Left 1 · Middle 2 · Right 3 · Back 8 · Forward 9 | 4–7 are scroll-wheel steps and aren't offered as clicks |
| Keyboard key | Any physical key, plus optional Shift/Ctrl/Alt/Super | Stored as the **keycode** captured when you press the key in the key picker, plus its name for display |
| Hold (ms) | 0–200, default **10** | 0 shows "instant click", like Pulse |
| Click at | **Pointer** (default) · Watched spot | Watched spot warps the pointer, clicks, and optionally warps back |

**Keycodes, not characters.** A keycode identifies a physical key and doesn't change when you switch keyboard layouts, which is also how games treat keys. Modifiers are pressed before the key and released after it. Glint listens for `MappingNotify` to refresh key names when the keymap changes.

**Why the default hold is 10 ms rather than 0.** Event-driven apps see a 0 ms click without trouble, but apps that poll button or key *state* once per frame can miss a press and release that both happen between two of their frames. The press goes out the instant the trigger fires; the hold only delays the release, so it never affects response time.

**Release scheduling.** The hot loop releases the input as soon as its deadline passes (checked every iteration, so at most 0.5 ms late in Balanced). Don't use XTest's own delay field: the X server implements it by putting the whole client connection to sleep.

**Where the click lands.** *Pointer* is a normal click wherever the pointer is; in a game that captures the mouse, that's already the center. *Watched spot* moves the pointer to the sample point first (one extra request, about 10 µs). Avoid it in games that use relative mouse movement, because warping the pointer can jerk the camera.

**Grab-proof input.** The input connection calls `XTestGrabControl(impervious = true)`, so a server grab by another client (a window manager in the middle of a move, for example) can't stall a click.

**No stuck input, ever.** Glint releases held input on Stop, emergency stop, focus loss, safety stop, target lost, `SIGTERM`/`SIGINT`/`SIGHUP` and panics (through connection D), and performs defensive releases at startup. Validation caps the hold at 200 ms, far below X's key auto-repeat delay (about 600 ms by default), so a key can never start repeating.

**Known limitation.** XTest input is produced inside the X server, so the rare program that reads `/dev/input` directly won't see it. That's acceptable for an X11 tool, and no workaround is planned.

---

## 7. Every way an extra click can happen and what stops it

| # | Way an extra click could happen | What stops it | Test (§16) |
|---|---|---|---|
| 1 | The color flickers at the edge of the range | Tight/wide hysteresis + pixel agreement | `boundary-noise` |
| 2 | One-frame dropouts or a blinking animation during an appearance | Observed-absence re-arm window | `flicker` |
| 3 | A fade passes through the range | It's one appearance, so one click; exclude colors for hues it passes through | `fade` |
| 4 | A similar but wrong color | Tight start range + exclude list | `near-miss` |
| 5 | Pressing Start while the color is already showing | Every start begins latched | `arm-while-present` |
| 6 | The window moves or resizes, or monitors change | Epoch restart, which begins latched | `move` |
| 7 | The window comes back to the front with the color showing | Regaining focus counts as a restart | `focus` |
| 8 | You edit the range while running | Lookup-table swap counts as a restart | `live-edit` |
| 9 | An engine stall followed by a late check | Absence counted only from observed checks; a stall between checks doesn't count as absence | `stall` |
| 10 | Two engines on the same display | Single-instance lock | `two-engines` |
| 11 | The click itself makes the color blink (a feedback loop) | Safety stop on clicks per second | `feedback` |
| 12 | A stuck key auto-repeating | 200 ms hold cap + guaranteed releases | `kill-mid-press` |
| 13 | The action key is also a hotkey, or Glint hears its own fake keys | Config validation + ignoring XTest-sourced events | `action-equals-hotkey` |
| 14 | Glint's own window covers the watched spot (its live swatch *is* the target color) | Overlap warning and optional auto-hide | `ui-overlap` |
| 15 | Two real clicks 300 ms apart are read by the app as a double-click | Not an extra click; raise `min_fire_interval_ms` above the app's double-click time if it matters | documented |

---

## 8. Windows, monitors and the desktop environment

**Picking a window** works like Pulse's *Pick window*:

1. The UI sends `pick_window`. The engine grabs the pointer on the root window with a crosshair cursor (glyph 34 of the core cursor font) and grabs the Escape key.
2. You click a window. The `ButtonPress` names the top-level window under the pointer; the engine searches its subtree for the window carrying `WM_STATE` (the client) and makes that the target. Escape, a right-click or a 10 s timeout cancels. The grabs are always released, even on errors.
3. The engine reports the title (`_NET_WM_NAME`, falling back to `WM_NAME`), class (`WM_CLASS`), PID (`_NET_WM_PID`) and geometry, and the UI shows the title and status the way Pulse does ("Sober · Not in front").

**Identity and re-acquiring.** Window IDs change every time an app starts, so the target is stored by `WM_CLASS` (plus an optional "title contains"). When the target closes, Glint stops and shows "Closed — waiting for it to open". When a window with the same class appears again (through `_NET_CLIENT_LIST` changes or a newly mapped top-level), Glint re-acquires it automatically (a setting, on by default), but it never starts watching on its own.

**Tracking.** Glint selects `StructureNotify` on the client window and on its frame (reparenting window managers move the frame, not the client). On `ConfigureNotify` or `ReparentNotify` it re-translates coordinates, updates the control word and bumps the epoch. Unmap means hidden, map means visible, destroy means closed.

**"Only click in this window"** is on by default in window mode, as in Pulse. The engine watches `_NET_ACTIVE_WINDOW` on the root window through `PropertyNotify` and fires only while the active window is the target or its frame. With window managers that don't support EWMH, it falls back to `FocusIn`/`FocusOut` on the client. The status line mirrors Pulse: "In front" or "Not in front". An optional stricter check, *Pointer must be over the window*, does one `QueryPointer` at click time (about +0.1 ms).

**Compositor detection.** A running compositor owns the selection `_NET_WM_CM_S<screen>`. Glint watches that ownership through XFixes selection events, shows it in diagnostics, uses it to choose the backend, and shows the hint "With a compositor, screen mode sees changes one frame late."

**XWayland detection.** If `XDG_SESSION_TYPE=wayland`, `WAYLAND_DISPLAY` is set, or the server advertises the XWAYLAND extension, Glint shows "This is XWayland: only X11 apps can be watched and clicked."

**Screen lock.** The MIT-SCREEN-SAVER extension, plus `org.freedesktop.ScreenSaver.GetActive` on D-Bus when available, tells Glint when the screen is locked, and it stops while locked.

**Glint's own window.** The UI reports its window rectangle to the engine (`ui_geometry`). If that rectangle contains the watched spot, Glint warns "The Glint window covers the spot it's watching" and can auto-hide the live swatch, or the whole window, while started.

**Multiple monitors and scaling.** All coordinates are root-window pixels. RandR monitor rectangles already account for rotation, and fractional scaling with `xrandr --scale` still produces root pixels, so nothing special is needed.

**Remote X** (`ssh -X`) is unsupported: there's no shared memory and round trips take milliseconds. The engine warns when `DISPLAY` isn't local.

---

## 9. Hotkeys

**Defaults:** F7 start/stop · **F8 emergency stop** · F6 sample the color at the watched spot. F8 is deliberately the same key as Pulse's, so one key stops both apps. Everything is rebindable.

**How they work.** The engine announces XInput 2.2 support (`XIQueryVersion`) and selects raw key events (`XI_RawKeyPress`, `XI_RawKeyRelease`) on the root window. Since XInput 2.1, raw events are delivered regardless of grabs, so hotkeys keep working while a fullscreen game holds the keyboard. Modifier state is tracked from the same raw events. Raw events don't consume the key, so the focused app also receives it; pick keys the app doesn't use. An optional **Capture** mode uses `XGrabKey` instead (registered for every NumLock/CapsLock/ScrollLock combination) so the key doesn't reach the app, but a passive grab can't fire while another program holds an active keyboard grab, and the UI explains that trade-off.

**Ignoring Glint's own input.** Raw events whose source device is one of the "Virtual core XTEST" devices are ignored, so the action key can never trigger a hotkey.

**Mode** mirrors Pulse's Mode row: **Press to toggle** (default) or **Hold to start** (watching only while the key is held; releasing it stops and releases input). Starting from a hotkey follows rule R7: if the color is already showing, Glint waits for it to leave first.

**Emergency stop** stops watching, releases everything, and ignores Start for 500 ms so a bouncing key can't restart it.

**Blocked combinations:** Ctrl+Alt+F1…F12 (they switch virtual terminals) and Ctrl+Alt+Backspace (it can kill the X server).

Hotkeys live in the engine, so they keep working with the UI closed, which is what makes "Keep running" useful.

---

## 10. IPC protocol

**Transport.** A Unix stream socket at `$XDG_RUNTIME_DIR/glint/glintd-<display>.sock` in a directory with mode 0700 (fallback `/tmp/glint-<uid>/`, refused if it exists and isn't owned by you). The engine checks `SO_PEERCRED` and only accepts connections from the same user. There are no network sockets.

**Framing.** Newline-delimited JSON: one object per line, at most 1 MiB, with a `"type"` tag and an optional `"id"` that replies echo back. JSON is the right trade here because the traffic is tiny (a few dozen messages per second at most) and you can debug it by hand with `socat`.

**Handshake**

```json
→ {"id":1,"type":"hello","protocol":1,"client":"ui","version":"0.1.0"}
← {"id":1,"type":"hello_ack","protocol":1,"engine_version":"0.1.0",
   "caps":{"xtest":"2.2","shm":true,"damage":true,"composite":true,"randr":"1.6","xi2":"2.4"},
   "session":{"display":":0","xwayland":false,"compositor":"picom","realtime":"rtkit (rr 10)"}}
```

**Requests (client → engine)**

| Type | Fields | Reply or events |
|---|---|---|
| `hello` | `protocol`, `client` (`ui`, `ctl` or `lab`), `version` | `hello_ack` or `error` |
| `get_state` | — | `state` |
| `subscribe` | `telemetry_hz` (0–10), `live_hz` (0–20) | Streams of `telemetry` and `live` |
| `set_profile` | `profile` (complete object) | `profile_applied {rev}` or `error {field}` |
| `use_profile` · `list_profiles` | `name` | `state` · `profiles` |
| `set_settings` | `settings` (close policy, hotkeys, engine options) | `settings_applied` |
| `start` · `stop` · `toggle` · `emergency_stop` · `release_all` | — | `state` |
| `pick_window` | — | `pick_started`, then `window_picked` or `pick_cancelled` |
| `pick_color` | — | `pick_started`, `magnifier` frames, then `color_picked` or `pick_cancelled` |
| `cancel_pick` | — | `pick_cancelled` |
| `sample_center` | — | `color_sampled {rgb, oklch}` |
| `record_palette` | `ms` | `palette {entries}` |
| `classify` | `colors` (up to 256) | `classified {verdicts}` (powers the UI's shade strips and "Check hex") |
| `run_lab` | `scenario`, `count` | `lab_progress`, `lab_result` |
| `ui_geometry` | `x`, `y`, `w`, `h` | — (used for the overlap warning) |
| `detach` · `quit` · `ping` | — | `bye` · `bye` · `pong` |

**Events (engine → client)**

| Type | Contents |
|---|---|
| `state` | Started or not, phase, target (kind, name, status: `in_front` / `not_in_front` / `hidden` / `closed`), backend, checks per second, profile name and revision, last stop reason |
| `telemetry` | Checks/s, capture round trip p50/p99, response p50/p99/max, clicks, suppressed, stalls, dropped events, CPU % |
| `live` | The color at the watched spot right now and its verdict |
| `fired` | Click count and engine-side latency (coalesced to at most 20 per second) |
| `magnifier` | Width, height, RGBA pixels (base64) and the center color |
| `warning` · `error` | Stable `code`, human-readable message, optional `field` |

**Rates and backpressure.** Telemetry is sent at most 10 times a second and the live swatch at most 20 times a second, and only while a UI is subscribed and visible. Each client has an outbound queue of 256 messages; when it fills, the oldest telemetry and live messages are dropped, and the controller never blocks. The hot thread is three hops away from any socket (ring → telemetry thread → controller), so no client can ever slow it down.

**Versioning.** `protocol` is an integer. Adding fields or message types keeps the number; breaking changes bump it. On a mismatch the UI offers **Restart engine**.

**Debugging by hand**

```sh
socat - UNIX-CONNECT:"$XDG_RUNTIME_DIR/glint/glintd-0.sock"
{"type":"hello","protocol":1,"client":"ctl","version":"dev"}
{"type":"get_state"}
```

**`glint-ctl`** wraps the protocol: `status`, `start`, `stop`, `toggle`, `panic`, `release-all`, `profile list`, `profile use <name>`, `profile show`, `profile export <name>`, `profile import <file>`, `pick-window`, `sample-center`, `diag`, `lab <scenario> [--count N]`, and `quit`.

---

## 11. Settings and profiles

| File | Contents |
|---|---|
| `~/.config/glint/settings.toml` | Close policy, active profile, hotkeys and hotkey mode, real-time and CPU options, UI state such as whether *More options* is open |
| `~/.config/glint/profiles/<name>.toml` | Target, sampling, colors, trigger, action, guards (example in Appendix B) |
| `~/.local/state/glint/glintd.log` | Engine log, rotated daily, last 5 kept |

**Ownership.** The engine is the only writer, so headless use, the CLI and hotkeys all see the same state. The UI edits through `set_profile` and `set_settings`. Every change saves automatically: writes are debounced (300 ms after the last change) and atomic (write a temporary file, `fsync`, rename), so holding a stepper button doesn't hammer the disk.

**Defaults and typos.** Every section and field has a default (the values in Appendix B; a new profile watches the screen center and has no color yet), so a hand-written file needs only `schema_version` and `name`. Unknown fields are errors, so a typo can't silently fall back to a default (ADR 0008).

**Validation.** The engine rejects bad values with an `error` naming the field, and the UI shows it inline next to that field. Warnings ("Gone after" under 15 ms, Max speed, a gray color, a hold that sets the real minimum gap, an exclusion that cuts into the range) are shown but don't block the change. A valid profile without a color, or in window mode without a window, can't be started yet.

| Rule | Why |
|---|---|
| Checks per second 100–10,000, or `max` | Sane pacing |
| `hold_ms` 0–200 | Stay far below key auto-repeat |
| `k_stay ≤ k_enter ≤` pattern size | Enter must imply Present |
| `exit_margin` 1.0–3.0; all tolerances > 0 (`tol_rgb` may be 0: exact match) | The wide range must contain the tight range |
| `rearm_absent_ms` 0–2000 (warning below 15) | Below 15 ms, flicker can count as a new appearance |
| `max_clicks_per_sec` 1–100 | The safety stop must always exist |
| Hotkeys distinct; action key ≠ any hotkey; no Ctrl+Alt+F1…F12 or Ctrl+Alt+Backspace | §9 |
| Offset keeps the whole pattern on the monitor or inside the window (checked at Start; ±16,384 before that) | §5.1 |
| Name 1–64 characters, no control characters; at most 16 include and 32 exclude colors | Sane files and table builds |
| Tolerances at most ΔL 1, ΔC 0.5, Δh 180°, ΔS and ΔV 1 | Beyond these a range already covers the whole axis |
| Hotkeys written `F7` or `Ctrl+Shift+F9`; `""` leaves start/stop or sample unbound; the emergency stop is always bound | §9, ADR 0008 |

**Migrations.** Every file carries `schema_version`. On upgrade the engine migrates in memory, one version at a time, writes a `.bak` copy of the original, and saves the new version. A file without `schema_version` is an error, and a file from a newer Glint is refused ("update Glint") rather than guessed at.

**Import and export.** Profiles are plain TOML; `glint-ctl profile export` and `profile import` copy them, and *More options* in the UI offers the same.

---

## 12. UI plan matched to Pulse

### 12.1 Reuse Pulse's Slint code first

Pulse is already Rust + Slint, which changes the best path. Rather than rebuilding its look from screenshots, move Pulse's theme and components into a small shared crate that both apps use. Pulse and Glint then stay identical as either one evolves, and Glint's UI work shrinks to laying out its own cards.

1. Create a kit crate (call it `slint-kit`, or anything you like) containing Pulse's `Theme` global, its fonts, and its components: card, section label, row, dropdown, key picker, stepper, segmented control, toggle, the button variants, the disclosure row, and the dialog overlay.
2. Point Pulse at the kit first and confirm with a screenshot comparison that it still looks pixel-identical. Only then build Glint on it.
3. Pin both apps to the same Slint version so the shared `.slint` files compile the same way in both.
4. Add Glint's new pieces (swatch, shade strip, magnifier) to the kit in Pulse's style; Pulse simply doesn't use them.

Because Glint is open source, the kit is published along with it, which makes those Pulse components public. You own that code, so license the kit MIT OR Apache-2.0: Glint can use it under GPLv3, and Pulse can keep using it whatever Pulse's own license is.

The kit crate exposes the path of its `ui/` folder, and each app hands that path to the Slint compiler as a library path, which makes `@kit/…` imports resolve:

```rust
// slint-kit/src/lib.rs
pub const SLINT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/ui");

// glint-ui-view/build.rs  (slint-kit is listed under [build-dependencies]; ADR 0002)
fn main() {
    let config = slint_build::CompilerConfiguration::new().with_library_paths(
        std::collections::HashMap::from([(
            "kit".to_string(),
            std::path::PathBuf::from(slint_kit::SLINT_DIR),
        )]),
    );
    slint_build::compile_with_config("ui/app.slint", config).unwrap();
}
```

```slint
// glint-ui-view/ui/app.slint
import { Theme, Card, Row, Stepper, Segmented, Toggle, PrimaryButton } from "@kit/index.slint";
```

Check the library-path section of the current Slint documentation for the exact API before wiring this up.

### 12.2 The visual language

These measurements come from your two screenshots (1920×1080, so they're close to logical pixels). Use them to verify the extracted kit, or as the spec for any piece that turns out to be missing from Pulse's source.

Pulse is a compact, single-column, monochrome dark panel. There's no accent color at all: white plays the accent role (primary buttons, the selected segment, a switched-on toggle), and every other element is a step on a neutral gray ramp that matches Tailwind's "neutral" scale. That gives Glint one striking opportunity: **the only color anywhere in Glint's interface is the color you're tracking.** Swatches, shade strips and the magnifier are the only chromatic pixels on screen, so your target color always stands out.

| Token | Hex | Used for in Pulse |
|---|---|---|
| `bg` | #0a0a0a | Window background |
| `surface` | #141414 | Cards |
| `surface-raised` | #191919 | Big Start button in its idle state |
| `field` | #1e1e1e | Dropdowns, steppers, segmented track, secondary buttons, dialog |
| `border` | #2c2c2c | Card and button borders (1 px) |
| `border-field` | #2a2a2a | Field borders (1 px) |
| `border-strong` | #4a4a4a | Dialog outline |
| `text` | #fafafa | Values, titles, button labels |
| `text-secondary` | #a3a3a3 | Row labels, status text, dialog body, text buttons such as "Cancel" |
| `text-muted` | #737373 | Section labels, helper text, disabled text |
| `text-warning` | #e5e5e5 | Inline ▲ warnings |
| `primary` / `on-primary` | #fafafa / #0a0a0a | Primary button, selected segment, toggle-on track / their text and the toggle knob |
| `scrim` | #000000 at about 60% | Modal backdrop |

| Text role | Size and weight | Color |
|---|---|---|
| Section label ("CLICKING") | ~11 px, semibold, uppercase, slight letter-spacing | `text-muted` |
| Row label, body text | ~13 px regular | `text-secondary` |
| Control values | ~13 px | `text` |
| Window name ("Sober") | ~14 px semibold | `text` |
| Big Start button | ~15 px semibold | depends on state |
| Dialog title | ~17 px semibold | `text` |
| Helper text | ~12–13 px | `text-muted` |

The typeface looks like the desktop's default sans (Noto Sans); check Pulse's source. If Pulse relies on the system font, bundle that font in the kit so both apps look identical on every distribution.

| Element | Measurement |
|---|---|
| Window | ~400 px wide, ~640 px of content, native window-manager title bar |
| Outer padding | 16 px left and right, 12 px top |
| Card | 368 px wide, 1 px border, corner radius ≈ 8–10 px, 14 px inner padding, 8 px between cards |
| Rows | 32 px tall controls on a 40 px pitch; label column ≈ 104 px |
| Section label to first row | ~12 px |
| Big Start button | 44 px tall, full card width |
| Buttons | 32 px tall, ~12 px horizontal padding |
| Toggle | 36 × 20 px track, 14 px knob |
| Dialog | 336 px wide, 16 px padding, buttons right-aligned |

### 12.3 Main window

```text
┌───────────────────────────── Glint ──────────────────────────────┐
│ ┌──────────────────────────────────────────────────────────────┐ │
│ │ [                         ▶  Start                         ] │ │
│ │ Switch to Sober and press F7 to start · F8 stops everything  │ │
│ └──────────────────────────────────────────────────────────────┘ │
│ ┌──────────────────────────────────────────────────────────────┐ │
│ │ COLOR                                                        │ │
│ │ Color           [■ #3BD16F         ]   [ Pick ]              │ │
│ │ Shades          ( Tight  [Normal]  Loose )                   │ │
│ │ Seeing          ■ #1E2A20 · no match                         │ │
│ │ Watch           ( Screen center  [Window center] )           │ │
│ └──────────────────────────────────────────────────────────────┘ │
│ ┌──────────────────────────────────────────────────────────────┐ │
│ │ CLICKING                                                     │ │
│ │ Click           [ Mouse button   ▾ ]  [ Left      ▾ ]        │ │
│ │ Hold (ms)       [ -   10   + ]  released after 10 ms         │ │
│ │ Mode            ( [Press to toggle]  Hold to start )         │ │
│ │ Start/stop key  [ F7     ▾ ]  Emergency stop: F8             │ │
│ └──────────────────────────────────────────────────────────────┘ │
│ ┌──────────────────────────────────────────────────────────────┐ │
│ │ GAME WINDOW                                                  │ │
│ │ Sober                                        [ Pick window ] │ │
│ │ In front                                                     │ │
│ │ (━●) Only click in this window                               │ │
│ └──────────────────────────────────────────────────────────────┘ │
│ ▸ More options                                                   │
│                                                                  │
│                                                   [ Quit Glint ] │
└──────────────────────────────────────────────────────────────────┘
```

Legend: `[ ]` field or button · `( )` segmented control with the selected option in `[brackets]` · `▾` dropdown · `(━●)` toggle switched on. Numbers are illustrative.

The hero card works exactly like Pulse's: one big button and a status line. **COLOR** is new and sits directly under it, because it's what makes Glint different; its *Seeing* row is the live swatch of what the engine sees at the watched spot. **CLICKING** and **GAME WINDOW** keep Pulse's rows and wording so the two apps feel like one family. *Mode* reads "Hold to start" instead of Pulse's "Hold to click", because holding the key makes Glint *watch*, not click. Everything else lives under **More options**, closed by default. In screen-center mode the GAME WINDOW card still matters: with *Only click in this window* on, clicks only happen while that window is in front.

### 12.4 More options

```text
▾ More options
┌──────────────────────────────────────────────────────────────┐
│ DETECTION                                                    │
│ Pattern         ( Pixel  [3×3]  5×5  Custom )                │
│ Must match      [ -   5   + ]  of 9 pixels                   │
│ Checks          ( Eco  [Balanced]  Max )  2,000/sec          │
│ Gone after      [ -  10   + ]  ms without the color          │
│ ▲ Under 15 ms, flicker can count as a new appearance.        │
│ Min gap         [ -  25   + ]  ms between clicks             │
│ Safety stop     [ -  15   + ]  clicks/sec                    │
│ Read from       [ Auto       ▾ ]  window buffer              │
│ Offset          X [ -  0  + ]   Y [ -  0  + ]                │
└──────────────────────────────────────────────────────────────┘
┌──────────────────────────────────────────────────────────────┐
│ COLOR RANGE                                                  │
│ Model           ( [OKLCh]  HSV  RGB )                        │
│ Lightness ±     [ -  10   + ]                                │
│ Colorfulness ±  [ -   5   + ]                                │
│ Hue ±           [ -   7   + ]  degrees                       │
│ Shade strip     ░░░░██████████░░  lightness range            │
│ Hue strip       ░░░░░░████░░░░░░  hue range                  │
│ Never match     ■ #6BD13B  ×      [ + Add ]                  │
│ [ Record 5 s ]   [ Sample center (F6) ]   [ Check hex ]      │
└──────────────────────────────────────────────────────────────┘
┌──────────────────────────────────────────────────────────────┐
│ ENGINE                                                       │
│ When closing    ( [Ask]  Keep running  Quit )                │
│ Real-time       on (rtkit)                                   │
│ Response        p50 0.42 · p99 0.91 ms · 2,000/sec           │
│ [ Run latency test ]  last: 1,000 runs · 0 extra             │
│ Profile         [ Sober — green flash  ▾ ]                   │
│ [ Copy diagnostics ]   [ Import ]   [ Export ]               │
└──────────────────────────────────────────────────────────────┘
```

The labels are plain words for the settings in §5: *Must match* is `k_enter` (with `k_stay` derived), *Gone after* is `rearm_absent_ms`, *Min gap* is `min_fire_interval_ms`, *Safety stop* is `max_clicks_per_sec`, and *Read from* is the capture backend. Each shade strip is a row of 16 swatches generated around your color and classified by the engine (`classify`): swatches that count are drawn at full color, the rest dimmed to 25%, so you can *see* where your range starts and stops.

### 12.5 Close dialog

```text
┌──────────────────────────────────────────────┐
│ Close Glint?                                 │
│                                              │
│ Keep it running in the background so F7 and  │
│ F8 keep working, or quit it completely.      │
│ You can open this window again from the apps │
│ menu.                                        │
│                                              │
│    Cancel   [ Keep running ]  [ Quit Glint ] │
└──────────────────────────────────────────────┘
```

It's an in-window overlay with a 60% black scrim, exactly like Pulse's. The hotkey names in the body text are bound to the current settings, so the sentence stays true if you rebind F7 or F8. *Cancel* is a text button, *Keep running* a secondary button, and *Quit Glint* the white primary button.

### 12.6 Component inventory

| Component | Pulse reference | States and behavior |
|---|---|---|
| Hero button | ▶ Start | Disabled (`surface-raised` + `text-muted`, e.g. no color chosen yet) · Ready (`field` + `text`) · Started ("■ Stop", white primary) |
| Status line | "Switch to Sober…" | `text-secondary`, centered, facts joined with " · " like Pulse |
| Card + section label | CLICKING, GAME WINDOW | — |
| Row | Click · Speed · Hold · Mode | Label column, controls, helper text |
| Dropdown | Keyboard key ▾ | Popup list styled like a field; keyboard navigable |
| Key picker | E ▾ · F ▾ | "Press a key…" capture state; stores the keycode and shows the key's name |
| Stepper | − 10 + | Click-and-hold repeats; typing a value works; clamps to its range |
| Segmented control | Press to toggle / Hold to click | Selected segment = white primary |
| Toggle | Only click in this window | On: white track, dark knob · Off: field track, muted knob · 120 ms slide |
| Buttons | Pick window · Quit Pulse · Keep running · Cancel | Primary, secondary and text variants |
| Inline warning | ▲ Under 100 ms can break Roblox chat. | `text-warning`, triangle glyph, never colored |
| Disclosure | ▸ More options | Turns into ▾ when open; the open state is remembered |
| Dialog overlay | Close Pulse? | Scrim, `field` fill, `border-strong` outline |
| Swatch (new) | — | 16 × 16 px, radius 3, 1 px border, hex value in `text` |
| Shade strip (new) | — | 16 cells; matching cells at full color, others dimmed to 25% |
| Magnifier (new) | — | 11 × 11 screen pixels at ×8, center cell outlined in white |
| Tooltip (new) | — | Slint's built-in `Tooltip` (1.17+), `field` fill, `text-secondary` |

### 12.7 Copy

Write the way Pulse does: short, plain, sentence case, status facts joined with " · ", warnings starting with ▲, and errors that say what happened and what to do next.

| Situation | Text |
|---|---|
| Idle, window mode | Switch to Sober and press F7 to start · F8 stops everything |
| Started | Watching the center of Sober · F8 stops everything |
| No color chosen | Pick a color to start |
| Warnings | ▲ Max keeps one CPU core busy. · ▲ Under 15 ms, flicker can count as a new appearance. · ▲ With a compositor, screen mode sees changes one frame late. · ▲ This is XWayland: only X11 apps can be watched and clicked. |
| Errors | Sober closed. Glint will find it again when it reopens. · Stopped: 16 clicks in 1 s — looks like a loop. |

### 12.8 Slint implementation notes

- **Files:** `ui/app.slint` (the window), `ui/cards/*.slint` (one file per card), `ui/dialogs.slint`; everything shared comes from `@kit`. They live in `glint-ui-view`, whose `build.rs` compiles them with `slint-build` and whose only Rust is the re-export of the generated components. Slint's generated code carries `allow(unsafe_code)`, which `#![forbid(unsafe_code)]` rejects, so all hand-written UI code lives in `glint-ui` under `forbid` (ADR 0002).
- **Window:** native decorations, like Pulse (it uses the window manager's title bar). Fixed width of 400 px; the content sits in a `Flickable` that only scrolls once *More options* is open. Set the XDG app id to `glint` so the window manager matches the window to `glint.desktop` (check the current Slint API for the app-id setter).
- **Renderer:** the software renderer keeps the UI completely off the GPU. Select it with `SLINT_BACKEND=winit-software` in the `.desktop` file, or with Slint's `BackendSelector` API.
- **Threads:** an IO thread reads the socket and hands each event to the UI with `slint::Weak::upgrade_in_event_loop`; telemetry is coalesced to at most 15 updates per second. Callbacks only enqueue requests and never block.
- **Closing:** `window().on_close_requested(...)` shows the overlay dialog and returns `CloseRequestResponse::KeepWindowShown` (Appendix A.6).
- **No About screen:** version and environment details live under *More options → Engine → Copy diagnostics*. Slint is used under GPLv3, which needs no on-screen credit (§2).
- **Accessibility:** every custom control sets `accessible-role` and `accessible-label`; Tab follows visual order; Space and Enter activate; arrow keys move segmented controls and steppers; focus shows as a visible 1 px ring in `text-secondary`.
- **Idle cost:** no animations except the 120 ms toggle and disclosure transitions. While minimized or hidden, the UI unsubscribes from telemetry and the live swatch, so it wakes up zero times per second.
- **Iterating on the look:** `slint-viewer --auto-reload ui/app.slint`, or the Slint VS Code extension's live preview, with Pulse open next to it.
- **HiDPI:** everything is in logical pixels; Slint applies the scale factor, and `SLINT_SCALE_FACTOR` overrides it.

### 12.9 Picking colors

- **Pick (from the screen):** the engine grabs the pointer with a crosshair cursor, and the COLOR card turns into a live magnifier (11 × 11 pixels at ×8, updated up to 30 times a second by the engine) with the hex value under the pointer. Click to choose; Escape or a right-click cancels. Picking stops watching first.
- **Sample center (F6):** adds the color that's at the watched spot right now. This is the practical way to capture a color that only appears briefly in a game, and it works with the UI closed.
- **Record 5 s:** collects every color seen at the watched spot for five seconds, groups near-identical ones, and shows the 12 most frequent as swatches. Click a swatch to include it, or mark it *Never match*.
- **Check hex:** paste a hex value to see whether it counts, using the very same lookup table as the engine (`classify`).

---

## 13. Repository layout

```text
glint/
├── Cargo.toml                    workspace root
├── rust-toolchain.toml           stable + rustfmt + clippy + musl target
├── deny.toml                     cargo-deny: licenses and advisories
├── LICENSE                       GPL-3.0 text (the kit ships its own MIT and Apache-2.0 files)
├── crates/
│   ├── glint-core/               pure logic, no I/O, #![forbid(unsafe_code)]
│   │   ├── src/color/            oklab.rs · hsv.rs · rule.rs · lut.rs
│   │   ├── src/                  pattern.rs · trigger.rs · profile.rs · settings.rs · validate.rs
│   │   └── benches/classify.rs
│   ├── glint-x11/                X11 backends; unsafe only in capture/shm.rs, with SAFETY comments
│   │   └── src/                  conn.rs · wire.rs (raw hot-path protocol, ADR 0003) · caps.rs · format.rs
│   │                             inject.rs · monitors.rs · windows.rs · picker.rs · hotkeys.rs · env.rs
│   │                             capture/{getimage,shm,window,damage}.rs
│   ├── glint-ipc/                protocol types, framing, client and server helpers
│   ├── glint-engine/             bin: glintd
│   │   └── src/                  main.rs · hot.rs · controller.rs · telemetry.rs · lifecycle.rs
│   │                             rt.rs · store.rs · lock.rs
│   ├── glint-ui-view/            lib: the Slint-generated view; its code needs allow(unsafe_code) (ADR 0002)
│   │   ├── build.rs              compiles ui/ with the kit's library path (§12.1)
│   │   └── ui/                   app.slint · dialogs.slint · cards/*.slint
│   ├── glint-ui/                 bin: glint; every hand-written UI line, #![forbid(unsafe_code)]
│   │   └── src/                  main.rs · client.rs · spawn.rs · bind.rs
│   ├── glint-ctl/                bin: glint-ctl
│   └── glint-lab/                bin: glint-lab + Xvfb integration tests
├── assets/                       glint.svg · glint.desktop · metainfo.xml
├── packaging/                    appimage/ · aur/PKGBUILD · deb/ · rpm/ · systemd/ · autostart/
├── docs/                         ARCHITECTURE.md · PROTOCOL.md · MEASUREMENTS.md · TUNING.md
│                                 TROUBLESHOOTING.md · adr/
└── .github/workflows/ci.yml
```

The shared Slint kit lives wherever suits both projects best: a sibling crate in one combined workspace, a git submodule, or a small published crate (§21).

Every source file starts with `// SPDX-License-Identifier: GPL-3.0-or-later` (the kit's files use `MIT OR Apache-2.0`).

---

## 14. Dependencies and Cargo workspace

| Crate | Used by | Why |
|---|---|---|
| `x11rb` (features `xtest shm damage composite randr xinput xfixes screensaver`) | x11 | Pure-Rust X11 protocol and extensions |
| `serde`, `serde_json`, `toml` | core, ipc | Profiles, settings, protocol |
| `arc-swap` | engine | Lock-free profile swaps for the hot thread |
| `rtrb` | engine | Wait-free event ring out of the hot thread |
| `triple_buffer` | engine | Wait-free live-swatch hand-off |
| `rayon` | core | Parallel lookup-table builds |
| `rustix`, `libc` | x11, engine | memfd, flock, prctl, scheduling, mlockall, clock_nanosleep |
| `memmap2` | x11 | Mapping the shared-memory segment |
| `signal-hook` | engine | Graceful SIGTERM/SIGINT/SIGHUP |
| `zbus` | engine | rtkit and screensaver over D-Bus, pure Rust |
| `tracing`, `tracing-subscriber`, `tracing-appender` | all | Logs (never from the hot loop) |
| `thiserror`, `anyhow` | libraries, binaries | Errors |
| `clap` | binaries | Command lines |
| `directories` | engine, ui | XDG paths |
| `slint`, `slint-build`, your `slint-kit` | ui | UI |
| `assert_no_alloc` (debug builds) | engine | Enforce zero allocation in the loop |
| `proptest`, `criterion` (dev) | core, engine | Property tests and benchmarks |

```toml
# Cargo.toml (workspace root)
[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "GPL-3.0-or-later"      # Slint is used under GPLv3 (§2); the shared kit crate stays MIT OR Apache-2.0

[workspace.dependencies]
x11rb = { version = "0.13.2", features = ["xtest", "shm", "damage", "composite", "randr", "xinput", "xfixes", "screensaver"] }
x11rb-protocol = { version = "0.13.2", default-features = false, features = ["std"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "1"
arc-swap = "1"
rtrb = "0.4"
triple_buffer = "9"
rayon = "1"
rustix = { version = "1", features = ["fs", "process", "thread", "time", "mm"] }
libc = "0.2"
memmap2 = "0.9"
signal-hook = "0.4"
zbus = "5"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
tracing-appender = "0.2"
thiserror = "2"
anyhow = "1"
clap = { version = "4", features = ["derive"] }
directories = "6"
# Exactly Pulse's Slint version and feature set, so the shared kit compiles identically (ADR 0001)
slint = { version = "=1.18.1", default-features = false, features = [
    "std", "compat-1-18", "backend-winit-x11", "renderer-femtovg", "renderer-software", "accessibility",
] }
slint-build = "=1.18.1"
assert_no_alloc = "1"
proptest = "1"
criterion = "0.8"
palette = "0.7"         # dev-dependency: OKLab test-vector cross-check (§5.4)

[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
debug = "line-tables-only"   # readable perf and flamegraph traces
panic = "abort"              # the engine's panic hook releases input first (§4.3)
```

```toml
# rust-toolchain.toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
targets = ["x86_64-unknown-linux-musl"]
```

Versions were updated on 2026-11-13 to the then-current majors, except `x11rb`, which stays on 0.13.2, and Slint, which is pinned to Pulse's exact version and features (ADR 0001). Keep `x11rb` free of its `allow-unsafe-code`/libxcb features so the engine stays fully static.

---

## 15. Development environment

Pulse already builds on your machine, so Slint's prerequisites are installed. For other machines and for the test tools:

```sh
# Arch and derivatives
sudo pacman -S --needed base-devel rustup pkgconf fontconfig libxkbcommon libxkbcommon-x11 \
  libx11 libxcursor libxrandr libxi mesa xorg-server-xvfb xorg-server-xephyr \
  xorg-xdpyinfo xorg-xwininfo xorg-xprop xorg-xev xorg-xinput socat

# Debian / Ubuntu
sudo apt install build-essential pkg-config libfontconfig1-dev libxkbcommon-dev libxkbcommon-x11-dev \
  libx11-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev xvfb xserver-xephyr \
  x11-utils xinput socat musl-tools

# Fedora
sudo dnf install gcc pkgconf-pkg-config fontconfig-devel libxkbcommon-devel libxkbcommon-x11-devel \
  libX11-devel libXcursor-devel libXrandr-devel libXi-devel mesa-libGL-devel \
  xorg-x11-server-Xvfb xorg-x11-server-Xephyr xdpyinfo xwininfo xprop xev xinput socat

# openSUSE
sudo zypper install gcc pkgconf fontconfig-devel libxkbcommon-devel libxkbcommon-x11-devel \
  libX11-devel libXcursor-devel libXrandr-devel libXi-devel Mesa-libGL-devel \
  xorg-x11-server-Xvfb xorg-x11-server-Xephyr xdpyinfo xwininfo xprop xev xinput socat
```

Package names drift between releases; if a build stops on a missing `.pc` file, install the matching `-dev` / `-devel` package.

```sh
# Rust toolchain and tools
rustup default stable
rustup component add rustfmt clippy
rustup target add x86_64-unknown-linux-musl
cargo install cargo-nextest cargo-deny flamegraph slint-viewer

# A safe sandbox: a nested X server in a window
Xephyr :2 -screen 1280x720 &
DISPLAY=:2 cargo run -p glint-engine
```

Develop the engine against Xephyr first: a bug there can't click into your real desktop.

---

## 16. Testing and verification

| Layer | What it covers | Where |
|---|---|---|
| Unit | Color math against test vectors, rules, patterns, state-machine transitions, validation, migrations | `glint-core` |
| Property (`proptest`) | The six invariants of §5.5 over random check streams with random timing; IPC encode/decode round trips | core, ipc |
| Exhaustive | The lookup table equals the reference math for all 16,777,216 colors (optimized build, a few seconds; `glint-core` is built with `opt-level = 3` even in dev, so it runs in every `cargo test`) | core |
| Fuzzing (`cargo-fuzz`) | The IPC line parser and the profile parser | ipc, core |
| Integration (Xvfb) | Engine + lab end-to-end scenarios below, correctness only | `glint-lab`, CI |
| Performance | Latency distributions, X server tax, CPU use | `glint-lab` on real hardware |
| Soak | 60 minutes of randomized scenarios | Real hardware, before every release |

**How `glint-lab` measures.** It opens its own window, makes that window Glint's target through the IPC (`set_profile`, then `start`), and warps the pointer into it so clicks land there. For each trial it fills the window's center with a color, follows the drawing request with a round trip so it knows the server has executed it, and timestamps that moment with `CLOCK_MONOTONIC`. It then timestamps the arrival of Glint's `ButtonPress` (or `KeyPress`) on its own window. The difference is the true response time including X delivery, and Glint's `fired` events split it into waiting, capture, decision, injection and delivery. The lab runs against the screen backend (with or without a compositor) and against the window-buffer backend.

Report format (illustrative numbers):

```text
glint-lab basic --count 1000 --speed balanced --backend screen
appearances 1000 · clicks 1000 · extra 0 · missed 0
response    p50 0.41 ms · p95 0.62 ms · p99 0.88 ms · max 1.73 ms
split       wait 0.24 · capture 0.09 · decide 0.001 · inject 0.02 · deliver 0.06 ms
server tax  other clients' round trips +0.04 ms at p99
verdict     PASS  (p99 ≤ 2 ms, max ≤ 10 ms, 0 extra, 0 missed)
```

| Scenario | What the lab does | Pass condition |
|---|---|---|
| `basic` | N appearances of the exact color, on for 30–500 ms and off for 60–600 ms at random | Exactly N clicks; latency report |
| `shades` | Appearances use random shades inside the tight range | Exactly N clicks |
| `near-miss` | Colors just outside the wide range, plus the exclude colors | 0 clicks |
| `distractor` | The target color appears just outside the sample pattern | 0 clicks |
| `flicker` | 1–2 frame dropouts inside appearances, shorter than *Gone after* | 1 click per appearance |
| `boundary-noise` | The color jitters between the tight and wide ranges | 1 click per appearance |
| `fade` | 300 ms fades in and out through intermediate colors | 1 click per appearance |
| `rapid` | Gaps just above, then just below *Gone after* | N clicks, then merged appearances as documented |
| `arm-while-present` | Start while the color is showing | 0 clicks until it leaves and returns |
| `move` | Move or resize the window so the color lands under the new center | 0 clicks caused by the move |
| `focus` | Focus another window mid-appearance, then refocus with the color showing | 0 clicks into other windows; 0 on refocus |
| `live-edit` | Change the range while running | 0 clicks caused by the edit |
| `stall` | `SIGSTOP` the engine for 30 ms around an edge, then `SIGCONT` | No extra click; a late click is flagged, not failed |
| `two-engines` | Launch a second `glintd` | It refuses to start (exit code 3) |
| `feedback` | The lab toggles its color on every click it receives | Safety stop within 1 s |
| `action-matrix` | Every mouse button and a set of keys, hold 0/10/50 ms | Exactly one press, then one release, for each |
| `kill-mid-press` | `SIGKILL` the engine during a hold, then restart it | Input released on restart |
| `action-equals-hotkey` | Configure the action key to be a hotkey | Validation rejects it |
| `ui-overlap` | Place the UI over the watched spot | Warning shown; no clicks caused by the swatch |
| `soak` | 60 minutes of everything above, randomized | 0 violations; flat memory |

**CI** (GitHub Actions on Ubuntu): `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo nextest run`, the exhaustive table test in release mode, the Xvfb integration suite (`xvfb-run -a -s "-screen 0 1280x720x24" cargo nextest run -p glint-lab --release --run-ignored all`), an optional job that also starts `picom --backend xrender` to cover the composited path, and `cargo deny check` (allow `GPL-3.0-only` and `GPL-3.0-or-later` in `deny.toml`, or Slint's license fails the check). CI checks correctness only and never asserts timing, because shared runners are far too noisy; latency is measured on real hardware.

**Hardware matrix before v1.0:**

- GPUs: Intel integrated (modesetting + glamor), AMD, NVIDIA proprietary.
- Desktops: no compositor (i3 or Openbox), picom with its glx and xrender backends, KWin on X11, GNOME on Xorg, Xfce with compositing on and off, and your own desktop.
- Displays: 60, 144 and 240 Hz; two monitors with different refresh rates; 4K at 2× scaling.
- Power: a laptop on battery with the powersave CPU governor.

**Manual checklist before each release:**

- [ ] *Pick window* works on your window manager, and the status flips between "In front" and "Not in front".
- [ ] Pressing Start while the color is showing produces no click until it leaves and comes back.
- [ ] F8 stops everything with the UI closed.
- [ ] *Keep running*, then reopening Glint from the apps menu, shows the same state.
- [ ] `kill -9` on the engine in the middle of a hold, then restarting it, leaves nothing stuck.
- [ ] Night light on: detection unchanged.
- [ ] Compositor on and off: the backend switches automatically and the hint appears.
- [ ] `glint-lab basic --count 1000`: 0 extra, 0 missed, p99 ≤ 2 ms.

---

## 17. Diagnostics and observability

- **Live metrics** (shown under *More options → Engine* and by `glint-ctl status`): checks per second (actual versus target), capture round trip p50/p99/max, engine response p50/p99/max, clicks, suppressed clicks, safety stops, stalls (count and worst), dropped telemetry events, the hot thread's scheduling policy and priority, CPU % per thread, the backend in use and why it was chosen, compositor, XWayland, and the last 20 warnings.
- **Copy diagnostics** in the UI and `glint-ctl diag` on the command line produce one text block for bug reports: versions (Glint, Slint, x11rb, kernel, X server vendor and release), capabilities, session info, the active profile, the metrics above, and recent warnings.
- **Logs:** `~/.local/state/glint/glintd.log`, rotated daily with the last 5 kept; the level comes from settings or `RUST_LOG`. The hot thread never logs; it pushes events into the ring and the telemetry thread logs on its behalf.
- **Privacy:** Glint never writes screen content to disk or sends it anywhere. The only colors it saves are the ones you chose; the magnifier and the live swatch exist only in memory, and there is no network code at all.

---

## 18. Packaging and distribution

| Artifact | How it's built | Runs on |
|---|---|---|
| `glintd`, `glint-ctl`, `glint-lab` | `--target x86_64-unknown-linux-musl`, fully static (pure-Rust X11 and D-Bus) | Any x86_64 Linux with an X server |
| `glint` (UI) | glibc build on an old baseline (an older-distro container, or `cargo zigbuild --target x86_64-unknown-linux-gnu.2.31`) | Any distribution with a normal X11 desktop |
| AppImage | All four binaries + `.desktop` file + icon | Portable single file |
| AUR `glint` / `glint-bin` | PKGBUILD from source / from release binaries | Arch and derivatives |
| `.deb` / `.rpm` | `cargo-deb` / `cargo-generate-rpm` | Debian and Ubuntu / Fedora and openSUSE |
| aarch64 builds | Same recipes | Later |

**Install layout:** `/usr/bin/glint`, `/usr/bin/glintd`, `/usr/bin/glint-ctl`, `/usr/share/applications/glint.desktop`, `/usr/share/icons/hicolor/scalable/apps/glint.svg`, and optionally `/usr/lib/systemd/user/glintd.service`. The UI looks for the engine next to its own executable first, then on `$PATH`.

**License:** every package and the AppImage include the `LICENSE` file (GPL-3.0), and `glint --version`, `glintd --version` and `glint-ctl diag` print the license and the source repository URL, so anyone who gets a binary knows where the source lives.

**UI runtime libraries** (present on any X11 desktop): libX11, libXcursor, libXrandr, libXi, libxkbcommon and libxkbcommon-x11, fontconfig and freetype. The GPU renderers would add libGL/EGL; the software renderer avoids them.

```ini
# glint.desktop
[Desktop Entry]
Type=Application
Name=Glint
Comment=Click the moment a color appears
Exec=env SLINT_BACKEND=winit-software glint
Icon=glint
Categories=Utility;
StartupWMClass=glint
Terminal=false
```

**Autostart** is off by default and offered under *More options*. The recommended way is XDG autostart: `~/.config/autostart/glintd.desktop` with `Exec=glintd`. A systemd user unit also works, but it needs `DISPLAY` and `XAUTHORITY` in the systemd user environment (`systemctl --user import-environment DISPLAY XAUTHORITY`, which many X11 sessions already do):

```ini
# glintd.service (optional alternative)
[Unit]
Description=Glint engine
PartOf=graphical-session.target
After=graphical-session.target

[Service]
ExecStart=/usr/bin/glintd
Restart=on-failure

[Install]
WantedBy=graphical-session.target
```

---

## 19. Milestones

Effort sizes are relative: S is a few days, M about a week or two, L longer, for one developer.

### Phase 0 — Measure before building (S)

- [ ] `spike-capture`: 1,000,000 `GetImage` calls for a 3×3 patch on the root window and on an app window; record p50/p99/max with and without a compositor; repeat with MIT-SHM.
- [ ] Server tax: a second client times `GetInputFocus` round trips while the spike checks at 500/s, 2,000/s and max speed.
- [ ] `spike-inject`: an XTest click into a test window; time from sending to arrival.
- [ ] `spike-ui`: Pulse's components in a new 400 px window with the hero card and the close-dialog overlay.
- **Exit:** numbers recorded in `docs/MEASUREMENTS.md`; the default speed preset and backend confirmed or changed.

### Phase 1 — `glint-core` (M)

- [ ] OKLab/OKLCh, HSV and RGB rules; include and exclude colors; hysteresis; achromatic guard.
- [ ] 2-bit lookup-table builder (rayon) + the exhaustive equality test.
- [ ] Sample patterns and pixel agreement.
- [ ] Trigger state machine + the six invariants as property tests.
- [ ] Profile and settings types, validation, migrations.
- [ ] `criterion` benchmark for classification (target: under 100 ns for 9 pixels).
- **Exit:** `cargo test -p glint-core` is green, including the exhaustive and property tests.

### Phase 2 — Engine MVP (M)

- [ ] X connections A–D, capability probe, pixel-format decoding.
- [ ] Monitor center through RandR; the `screen` backend.
- [ ] XTest injector (mouse and keys), timed release, defensive release, panic hook, signal handling.
- [ ] Hot loop, speed presets, control word, epoch restarts, event ring, telemetry thread.
- [ ] IPC server (newline-delimited JSON) and `glint-ctl`.
- [ ] Single-instance lock; settings store with debounced atomic writes.
- [ ] Hotkeys through XI2 raw events (F7/F8/F6) with the XTEST-source filter.
- **Exit:** from the command line alone, Glint clicks once per appearance on a real desktop, and `basic`, `near-miss` and `arm-while-present` pass under Xvfb.

### Phase 3 — Lab and CI (M)

- [ ] `glint-lab`: test window, timing, report, every correctness scenario.
- [ ] GitHub Actions: fmt, clippy, nextest, exhaustive test, Xvfb integration, cargo-deny.
- **Exit:** every scenario in §16 that doesn't need real hardware passes in CI.

### Phase 4 — UI on Pulse's kit (M)

- [ ] Extract Pulse's theme and components into the shared kit; Pulse stays pixel-identical.
- [ ] Hero card, COLOR, CLICKING and GAME WINDOW cards; the *Quit Glint* button.
- [ ] Connect to or spawn the engine, handshake, subscriptions, live swatch.
- [ ] Close dialog and policies (Ask / Keep running / Quit), engine-crash handling.
- [ ] Color picking with the magnifier, *Sample center*, key picker.
- **Exit:** everything in the main window works without the CLI, and the UI uses about 0% CPU while hidden.

### Phase 5 — Window mode and guards (M)

- [ ] Pick window, client lookup, identity by `WM_CLASS`, automatic re-acquiring.
- [ ] Tracking through `StructureNotify`; epoch bump on move and resize.
- [ ] *Only click in this window* (EWMH + fallback) with "In front" / "Not in front".
- [ ] `window-buffer` backend + self-check; compositor detection; XWayland banner; stop on screen lock; UI-overlap warning.
- **Exit:** `move`, `focus` and `ui-overlap` pass, and the window-buffer backend is verified on your desktop with Sober.

### Phase 6 — Performance and advanced detection (M/L)

- [ ] rtkit real-time priority, timer slack, prefault and mlock, optional CPU pinning.
- [ ] `screen-shm` backend; `damage` backend with its self-healing fallback.
- [ ] *More options*: Detection, Color range (shade strips, exclusions, *Record 5 s*, *Check hex*), Engine (diagnostics, latency test, profiles).
- [ ] *Hold to start* mode; *Capture* hotkey mode.
- **Exit:** on your hardware, `glint-lab basic --count 10000` at Balanced shows p99 ≤ 2 ms and max ≤ 10 ms, with 0 extra and 0 missed.

### Phase 7 — Packaging and release (S/M)

- [ ] Static musl engine, baseline-glibc UI, AppImage, AUR, `.deb` and `.rpm`.
- [ ] `.desktop` file, icon, optional autostart.
- [ ] README, TUNING (compositor settings, rtkit, CPU governor), TROUBLESHOOTING.
- [ ] `LICENSE` in every package and the AppImage; license line and source URL in `--version` and diagnostics.
- [ ] Hardware matrix + 60-minute soak.
- **Exit:** the v1.0 definition of done below.

**Definition of done for v1.0:** every lab scenario passes 10,000 trials on at least two GPU vendors; engine response p99 ≤ 2 ms at Balanced and never above 10 ms; zero extra clicks and zero missed appearances; UI idle CPU about 0% while hidden; engine CPU at Balanced measured and documented (budget ≤ 10% of one core); every stuck-input test passes (SIGTERM, SIGINT, panic, SIGKILL + restart).

---

## 20. Risks and mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| `GetImage` is slow on some drivers (GPU readback sync on glamor or NVIDIA) | Confirmed on NVIDIA (Phase 0) | +0.45 ms p99 to other clients at Balanced | Speed presets; DAMAGE backend (fewer GetImages). SHM doesn't help at 3×3 |
| The X server stalls 10–18 ms about once a minute (other clients, GPU syncs), whatever Glint does | High (seen in every Phase 0 run) | A check landing in a stall exceeds the 10 ms ceiling | Nothing in Glint can shorten it; `glint-lab` reports stall-coincident trials separately (ADR 0004) |
| A compositor adds a frame in screen mode | High | 7–17 ms before Glint can see the color | `window-buffer` backend; tuning guide (unredirect fullscreen windows) |
| DAMAGE misses updates on some page-flip paths | Medium | Late or missed clicks in damage mode | Safety checks + automatic fallback to polling |
| `window-buffer` reads the wrong surface (child windows, Wine, container-based clients) | Medium | Blank reads | Self-check + fallback to `screen` |
| No rtkit, or real-time priority denied | Medium | Worse p99 under heavy load | Fallback + UI notice + limits.d instructions in TUNING |
| An app ignores XTest input | Low | No clicks reach it | Test your target apps in Phase 0; documented |
| Keyboard layout changes | Low | Wrong key | Keycode-based actions |
| Hotkeys collide with app keys | Medium | Annoyance | Rebindable; *Listen* vs. *Capture* modes |
| Running under XWayland | Medium | Wayland-native apps are invisible | Detection banner |
| Laptop battery drain | Medium | Shorter battery life | Eco preset; stop while the window isn't in front |
| Extracting the kit breaks Pulse | Low | A Pulse regression | Pixel-compare Pulse before and after (Phase 4) |
| A game's rules forbid automation | Depends on the game | Account bans | Scope note at the top of this plan; your call per game |

---

## 21. Decisions worth revisiting

1. **Name.** "Glint" is a placeholder.
2. **License — decided.** Glint is open source: Slint is used under GPLv3 and the project is GPL-3.0-or-later, while the shared kit stays MIT OR Apache-2.0. If you'd rather keep the engine crates permissive, MIT OR Apache-2.0 everywhere except the UI also works, because only the `glint` binary links Slint.
3. **Where the shared kit lives.** One combined workspace for Pulse and Glint, a git submodule, or a small published crate.
4. **Default hotkeys.** F6/F7/F8, with F8 shared with Pulse on purpose. If Glint's emergency stop shouldn't affect Pulse, change one of them.
5. **The 40 ms *Gone after* default.** Right for most content; lower it for content that legitimately disappears and comes back faster.
6. **DAMAGE as the default backend.** Decide after the Phase 6 measurements.
7. **One process like Pulse versus two.** If Pulse keeps running by hiding its window inside a single process, Glint deliberately differs (§2, §4). The behavior you see is the same, but the engine's timing can never be disturbed by the UI.

---

## Appendix A. Code skeletons

These are starting points, not drop-in code: the names follow this plan and the signatures follow `x11rb` 0.13 and Slint 1.18, but compile them against the current docs.

### A.1 Trigger state machine (`glint-core/src/trigger.rs`)

Implemented in Phase 1; the source and its tests are the reference (ADR 0007). Its shape:

```rust
pub enum Verdict { Absent, Present, Enter }            // from the sample pattern (§5.3); Enter implies Present
pub enum Step { None, Fire, Suppressed, SafetyStop }

impl Trigger {
    pub fn start(&mut self);   // manual Start: Latched (Idle with fire_if_present_on_start); no-op while started
    pub fn restart(&mut self); // epoch change: Latched with a fresh absence count; no-op while stopped
    pub fn pause(&mut self);   // the target window left the front; a restart resumes
    pub fn stop(&mut self);    // only a manual Start leaves Stopped, also after a safety stop
    pub fn step(&mut self, now: Instant, v: Verdict) -> Step;
}

// Latched + Absent: count only absence that was actually observed.
if let Some(last) = absence.last {
    absence.observed += now.saturating_duration_since(last).min(cfg.max_check_gap); // a stall isn't absence
}
absence.checks += 1;
absence.last = Some(now);
if absence.checks >= cfg.rearm_absent_samples && absence.observed >= cfg.rearm_absent {
    self.phase = Phase::Idle;
}

fn fire(&mut self, now: Instant) -> Step {
    self.phase = Phase::Latched(Absence::default()); // 1. consume the appearance before anything else
    if self.last_fire.is_some_and(|t| now.saturating_duration_since(t) < self.cfg.min_fire_interval) {
        return Step::Suppressed;                     // 2. the floor, at least hold_ms + 1
    }
    if self.history.last_second(now) >= self.cfg.max_clicks_per_sec {
        self.phase = Phase::Stopped;                 // 3. fixed 100-entry history: never allocates
        return Step::SafetyStop;
    }
    self.history.push(now);
    self.last_fire = Some(now);
    Step::Fire
}
```

`Hold` (same file) keeps the press and release timing (`press`, `due`, `release`) that the injector follows,
so invariant 6 is tested without an X server.

### A.2 Hot loop (`glint-engine/src/hot.rs`)

```rust
// `Hot` owns: control (shared AtomicU64), profiles (ArcSwap<CompiledProfile>),
// profile (current Arc), capture (Box<dyn Capture>), injector, trigger, events (rtrb producer),
// preview (triple-buffer input) and pacer. Both the controller and the hot thread update the
// control word with compare-and-swap; the hot thread only ever clears `armed` (and bumps epoch).
pub fn run_hot(mut h: Hot) -> ! {
    rt::set_timerslack_ns(1);
    let mut seen_epoch = u16::MAX;
    let mut was_armed = false;
    let mut px = Pixels::new(); // fixed capacity (64), never reallocates
    let mut next = Instant::now();
    loop {
        let ctl = Control::unpack(h.control.load(Ordering::Acquire));

        // Restart cleanly whenever the controller changed something (rule R7).
        if ctl.epoch != seen_epoch {
            seen_epoch = ctl.epoch;
            h.profile = h.profiles.load_full();
            h.trigger.set_cfg(h.profile.trigger);
            let _ = h.injector.release_now();
            // Every (re)start begins latched, so no capture is needed to decide (ADR 0007).
            match (was_armed, ctl.armed) {
                (false, true) => h.trigger.start(),  // a manual Start
                (true, true) => h.trigger.restart(), // moved, resized, profile swapped, back in front
                (_, false) => h.trigger.stop(),
            }
            was_armed = ctl.armed;
            next = Instant::now();
        }
        if !ctl.armed {
            std::thread::park_timeout(Duration::from_millis(50)); // the controller unparks us
            continue;
        }

        let t_req = Instant::now();
        if let Err(e) = h.capture.grab(h.profile.rect_at(ctl.origin), &mut px) {
            h.fail(e); // release input, request a stop with a reason, push an event
            was_armed = false; // we stopped ourselves: the next Start is a manual Start
            continue;
        }
        let t_cap = Instant::now();
        let verdict = h.profile.classify(&px); // table lookups + pixel agreement

        let _ = h.injector.service(t_cap); // a due release goes out before this check can fire (ADR 0007)
        match h.trigger.step(t_cap, verdict) {
            Step::Fire => {
                let _ = h.injector.press(t_cap);
                h.events.push_fire(t_req, t_cap, Instant::now());
            }
            Step::Suppressed => h.events.push_suppressed(t_cap),
            Step::SafetyStop => {
                let _ = h.injector.release_now();
                h.request_stop(StopReason::SafetyStop); // clears `armed` and bumps the epoch
                was_armed = false; // even if a Start lands before the next iteration, it's a manual Start
            }
            Step::None => {}
        }
        h.preview.publish(&px, verdict);   // wait-free hand-off for the UI swatch

        next += h.profile.period;
        let now = Instant::now();
        if now > next + h.profile.period * 4 {
            h.events.push_stall(now - next);
            next = now;
        }
        h.pacer.wait_until(next); // clock_nanosleep(TIMER_ABSTIME) + short spin; no-op in Max
    }
}
```

### A.3 GetImage capture and pixel decoding (`glint-x11/src/capture/getimage.rs`)

The request and reply logic below is written against x11rb for readability. The hot path sends the same GetImage over the allocation-free raw client instead (ADR 0003; reference implementation in `spikes/spike-capture`). The pixel decoding applies unchanged.

```rust
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, Drawable, ImageFormat};

pub struct GetImageCapture<C: Connection> {
    conn: C,
    drawable: Drawable, // the root window (screen mode) or the target window (window-buffer)
    fmt: PixelFormat,
}

impl<C: Connection> GetImageCapture<C> {
    pub fn grab(&mut self, r: Rect, out: &mut Pixels) -> Result<(), CaptureError> {
        let reply = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, self.drawable, r.x, r.y, r.w, r.h, !0)?
            .reply()?; // exactly one round trip
        self.fmt.decode(&reply.data, r.w, r.h, out);
        Ok(())
    }
}

/// Built once from the connection setup: visual masks, bits per pixel, scanline pad and
/// image byte order. Never assume BGRX. Visuals with fewer than 4 bits per channel are rejected.
pub struct PixelFormat { bpp: u32, pad: u32, msb_first: bool, r: Chan, g: Chan, b: Chan }

#[derive(Clone, Copy)]
struct Chan { shift: u32, bits: u32 }

impl Chan {
    fn from_mask(mask: u32) -> Self { Chan { shift: mask.trailing_zeros(), bits: mask.count_ones() } }
    fn to8(self, p: u32) -> u32 {
        let v = (p >> self.shift) & ((1u32 << self.bits) - 1);
        if self.bits >= 8 { v >> (self.bits - 8) } else { (v << (8 - self.bits)) | (v >> (2 * self.bits - 8)) }
    }
}

impl PixelFormat {
    fn stride(&self, w: u16) -> usize {
        (w as usize * self.bpp as usize).div_ceil(self.pad as usize) * self.pad as usize / 8
    }
    fn decode(&self, data: &[u8], w: u16, h: u16, out: &mut Pixels) {
        let stride = self.stride(w);
        out.clear();
        for y in 0..h as usize {
            let row = &data[y * stride..];
            for x in 0..w as usize {
                let p = self.read(row, x);
                out.push((self.r.to8(p) << 16) | (self.g.to8(p) << 8) | self.b.to8(p)); // 0xRRGGBB
            }
        }
    }
    fn read(&self, row: &[u8], x: usize) -> u32 {
        match self.bpp {
            32 => {
                let b = [row[x * 4], row[x * 4 + 1], row[x * 4 + 2], row[x * 4 + 3]];
                if self.msb_first { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) }
            }
            24 => {
                let b = &row[x * 3..x * 3 + 3];
                if self.msb_first { (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32 }
                else { (b[2] as u32) << 16 | (b[1] as u32) << 8 | b[0] as u32 }
            }
            16 => {
                let b = [row[x * 2], row[x * 2 + 1]];
                (if self.msb_first { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) }) as u32
            }
            _ => unreachable!("rejected at startup"),
        }
    }
}
```

### A.4 XTest injector (`glint-x11/src/inject.rs`)

Shown with x11rb for readability. Connection B sends these requests (FakeInput, GrabControl) over the allocation-free raw client (ADR 0003).

```rust
use std::time::Instant;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{Window, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, KEY_PRESS_EVENT, KEY_RELEASE_EVENT};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::CURRENT_TIME;

pub struct Injector<C: Connection> { conn: C, root: Window, action: Action, held: Option<Held> }
struct Held { release_type: u8, detail: u8, release_at: Instant }

impl<C: Connection> Injector<C> {
    pub fn new(conn: C, root: Window, action: Action) -> Result<Self, InjectError> {
        conn.xtest_grab_control(true)?; // impervious to other clients' server grabs
        Ok(Self { conn, root, action, held: None })
    }

    fn fake(&self, ty: u8, detail: u8) -> Result<(), InjectError> {
        self.conn.xtest_fake_input(ty, detail, CURRENT_TIME, self.root, 0, 0, 0)?;
        Ok(())
    }

    pub fn press(&mut self, now: Instant) -> Result<(), InjectError> {
        if self.held.is_some() { return Ok(()); } // never two presses without a release
        for &m in &self.action.modifiers { self.fake(KEY_PRESS_EVENT, m)?; }
        let (press, release, detail) = match self.action.kind {
            ActionKind::Mouse(button) => (BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, button),
            ActionKind::Key(keycode) => (KEY_PRESS_EVENT, KEY_RELEASE_EVENT, keycode),
        };
        self.fake(press, detail)?;
        self.conn.flush()?;
        self.held = Some(Held { release_type: release, detail, release_at: now + self.action.hold });
        Ok(())
    }

    pub fn service(&mut self, now: Instant) -> Result<(), InjectError> {
        if self.held.as_ref().is_some_and(|h| now >= h.release_at) { self.release_now()?; }
        Ok(())
    }

    pub fn release_now(&mut self) -> Result<(), InjectError> {
        if let Some(h) = self.held.take() {
            self.fake(h.release_type, h.detail)?;
            for &m in self.action.modifiers.iter().rev() { self.fake(KEY_RELEASE_EVENT, m)?; }
            self.conn.flush()?;
        }
        Ok(())
    }
}
```

### A.5 IPC message types (`glint-ipc/src/lib.rs`)

```rust
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct Envelope<T> {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(flatten)]
    pub body: T,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Hello { protocol: u32, client: ClientKind, version: String },
    GetState,
    Subscribe { telemetry_hz: u8, live_hz: u8 },
    SetProfile { profile: Profile },
    UseProfile { name: String },
    ListProfiles,
    SetSettings { settings: Settings },
    Start, Stop, Toggle, EmergencyStop, ReleaseAll,
    PickWindow, PickColor, CancelPick, SampleCenter,
    RecordPalette { ms: u32 },
    Classify { colors: Vec<Rgb> },          // Rgb serializes as "#RRGGBB"
    RunLab { scenario: String, count: u32 },
    UiGeometry { x: i32, y: i32, w: u32, h: u32 },
    Detach, Quit, Ping,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    HelloAck { protocol: u32, engine_version: String, caps: Caps, session: SessionInfo },
    State(StateSnapshot),
    Telemetry(TelemetrySnapshot),
    Live { rgb: Rgb, verdict: VerdictName },
    Fired { count: u64, latency_us: u32 },
    PickStarted,
    WindowPicked { window: WindowInfo },
    Magnifier { w: u8, h: u8, rgba_b64: String, center: Rgb },
    ColorPicked { rgb: Rgb },
    ColorSampled { rgb: Rgb, oklch: [f32; 3] },
    PickCancelled,
    Palette { entries: Vec<PaletteEntry> },
    Classified { verdicts: Vec<VerdictName> },
    Profiles { names: Vec<String>, active: String },
    ProfileApplied { rev: u64 },
    SettingsApplied,
    LabProgress { done: u32, total: u32 },
    LabResult(LabReport),
    Warning { code: String, message: String },
    Error { code: String, message: String, field: Option<String> },
    Pong,
    Bye,
}
```

### A.6 UI glue (`glint-ui/src/main.rs`)

```rust
#![forbid(unsafe_code)]
// The Slint-generated components come from glint-ui-view (ADR 0002).
use glint_ui_view::{AppWindow, ClosePolicy};

fn main() -> anyhow::Result<()> {
    // Keep the UI off the GPU: glint.desktop launches with SLINT_BACKEND=winit-software.
    let ui = AppWindow::new()?;
    let engine = client::connect_or_spawn()?; // §4.3: connect, otherwise spawn a detached glintd
    let tx = engine.sender();                 // non-blocking queue to the IO thread

    // Engine → UI, always applied on the Slint event loop.
    let weak = ui.as_weak();
    engine.on_event(move |ev| {
        let _ = weak.upgrade_in_event_loop(move |ui| bind::apply(&ui, ev));
    });

    // UI → engine: callbacks only enqueue; they never block the UI thread.
    ui.on_start_stop({ let tx = tx.clone(); move || tx.send(Request::Toggle) });
    ui.on_keep_running({
        let tx = tx.clone();
        move || { tx.send(Request::Detach); let _ = slint::quit_event_loop(); }
    });
    ui.on_quit_app({
        let tx = tx.clone();
        move || { tx.send(Request::Quit); let _ = slint::quit_event_loop(); }
    });

    // The window manager's close button opens the Pulse-style dialog instead of closing.
    let weak = ui.as_weak();
    ui.window().on_close_requested(move || {
        let ui = weak.unwrap();
        match ui.get_close_policy() {
            ClosePolicy::Ask => ui.set_show_close_dialog(true),
            ClosePolicy::KeepRunning => ui.invoke_keep_running(),
            ClosePolicy::Quit => ui.invoke_quit_app(),
        }
        slint::CloseRequestResponse::KeepWindowShown
    });

    ui.run()?;
    engine.flush_and_close(std::time::Duration::from_millis(500)); // make sure detach/quit got out
    Ok(())
}
```

### A.7 Slint pieces (reference only — prefer Pulse's own kit)

If Pulse's source already has these, use Pulse's versions; these sketches show the shape and the measured values.

```slint
// slint-kit/ui/theme.slint — values measured from Pulse; Pulse's own source wins if it differs
export global Theme {
    out property <color> bg: #0a0a0a;
    out property <color> surface: #141414;
    out property <color> surface-raised: #191919;
    out property <color> field: #1e1e1e;
    out property <color> border: #2c2c2c;
    out property <color> border-field: #2a2a2a;
    out property <color> border-strong: #4a4a4a;
    out property <color> text: #fafafa;
    out property <color> text-secondary: #a3a3a3;
    out property <color> text-muted: #737373;
    out property <color> text-warning: #e5e5e5;
    out property <color> primary: #fafafa;
    out property <color> on-primary: #0a0a0a;
    out property <color> scrim: #00000099;

    out property <length> fs-label: 11px;
    out property <length> fs-small: 12px;
    out property <length> fs-body: 13px;
    out property <length> fs-strong: 14px;
    out property <length> fs-hero: 15px;
    out property <length> fs-title: 17px;

    out property <length> radius-card: 9px;
    out property <length> radius-field: 6px;
    out property <length> pad-window: 16px;
    out property <length> pad-card: 14px;
    out property <length> gap-card: 8px;
    out property <length> control-h: 32px;
    out property <length> row-gap: 8px;
    out property <length> label-col: 104px;
}
```

```slint
// slint-kit/ui/components.slint (excerpt)
import { Theme } from "theme.slint";

export component Card inherits Rectangle {
    in property <string> title; // pass it uppercase, e.g. "CLICKING"
    background: Theme.surface;
    border-width: 1px;
    border-color: Theme.border;
    border-radius: Theme.radius-card;
    VerticalLayout {
        padding: Theme.pad-card;
        spacing: Theme.row-gap;
        if root.title != "": Text {
            text: root.title;
            font-size: Theme.fs-label;
            font-weight: 600;
            letter-spacing: 0.6px;
            color: Theme.text-muted;
        }
        @children
    }
}

export component Segmented inherits Rectangle {
    in property <[string]> options;
    in-out property <int> selected;
    callback changed(int);
    height: Theme.control-h;
    background: Theme.field;
    border-width: 1px;
    border-color: Theme.border-field;
    border-radius: Theme.radius-field;
    HorizontalLayout {
        padding: 2px;
        spacing: 2px;
        for option[i] in root.options: Rectangle {
            border-radius: Theme.radius-field - 2px;
            background: i == root.selected ? Theme.primary : transparent;
            Text {
                text: option;
                width: parent.width;
                height: parent.height;
                horizontal-alignment: center;
                vertical-alignment: center;
                font-size: Theme.fs-body;
                font-weight: i == root.selected ? 600 : 400;
                color: i == root.selected ? Theme.on-primary : Theme.text-secondary;
            }
            TouchArea {
                clicked => {
                    root.selected = i;
                    root.changed(i);
                }
            }
        }
    }
}

export component Toggle inherits Rectangle {
    in-out property <bool> checked;
    callback toggled(bool);
    width: 36px;
    height: 20px;
    border-radius: self.height / 2;
    background: root.checked ? Theme.primary : Theme.field;
    border-width: root.checked ? 0px : 1px;
    border-color: Theme.border-field;
    Rectangle {
        width: 14px;
        height: 14px;
        border-radius: 7px;
        y: (parent.height - self.height) / 2;
        x: root.checked ? parent.width - self.width - 3px : 3px;
        background: root.checked ? Theme.on-primary : Theme.text-muted;
        animate x { duration: 120ms; easing: ease-out; }
    }
    TouchArea {
        clicked => {
            root.checked = !root.checked;
            root.toggled(root.checked);
        }
    }
}
```

```slint
// glint-ui/ui/app.slint (excerpt: the close dialog overlay)
import { Theme, TextButton, SecondaryButton, PrimaryButton } from "@kit/index.slint";

export enum ClosePolicy { ask, keep-running, quit }

export component AppWindow inherits Window {
    title: "Glint";
    width: 400px;
    preferred-height: 640px;
    background: Theme.bg;
    default-font-size: Theme.fs-body;

    in-out property <ClosePolicy> close-policy: ClosePolicy.ask;
    in-out property <bool> show-close-dialog: false;
    in property <string> start-key: "F7";
    in property <string> stop-key: "F8";
    callback start-stop();
    callback keep-running();
    callback quit-app();

    // … hero card, COLOR, CLICKING, GAME WINDOW, More options, Quit Glint button …

    if root.show-close-dialog: Rectangle {
        background: Theme.scrim;
        TouchArea { } // swallow clicks behind the dialog
        Rectangle {
            width: 336px;
            height: content.preferred-height;
            x: (parent.width - self.width) / 2;
            y: (parent.height - self.height) / 2;
            background: Theme.field;
            border-width: 1px;
            border-color: Theme.border-strong;
            border-radius: Theme.radius-field;
            content := VerticalLayout {
                padding: 16px;
                spacing: 8px;
                Text {
                    text: "Close Glint?";
                    font-size: Theme.fs-title;
                    font-weight: 600;
                    color: Theme.text;
                }
                Text {
                    text: "Keep it running in the background so \{root.start-key} and \{root.stop-key} keep working, or quit it completely.";
                    wrap: word-wrap;
                    color: Theme.text-secondary;
                }
                Text {
                    text: "You can open this window again from the apps menu.";
                    wrap: word-wrap;
                    font-size: Theme.fs-small;
                    color: Theme.text-muted;
                }
                HorizontalLayout {
                    alignment: end;
                    spacing: 8px;
                    padding-top: 8px;
                    TextButton { text: "Cancel"; clicked => { root.show-close-dialog = false; } }
                    SecondaryButton { text: "Keep running"; clicked => { root.keep-running(); } }
                    PrimaryButton { text: "Quit Glint"; clicked => { root.quit-app(); } }
                }
            }
        }
    }
}
```

---

## Appendix B. Example profile

```toml
# ~/.config/glint/profiles/sober-green-flash.toml
schema_version = 1
name = "Sober — green flash"

[target]
mode = "window"                 # "screen" | "window"
monitor = "primary"             # screen mode: RandR monitor name or "primary"
window = { class = "sober", title_contains = "" }   # window mode: matched by WM_CLASS
offset = { x = 0, y = 0 }       # pixels away from the exact center

[sampling]
pattern = "square3"             # pixel | center2x2 | square3 | square5 | plus | custom
# plus_distance = 2             # plus: arm length in pixels (1–31)
# offsets = [[0, 0], [4, 0]]    # custom: up to 64 [dx, dy] offsets within ±31 px
k_enter = 5                     # of 9 pixels must be in the tight range to start (optional: §5.3 default)
k_stay = 3                      # of 9 pixels must be in the wide range to stay present (optional: ⌈k_enter / 2⌉)
speed = "balanced"              # eco (500/s) | balanced (2000/s) | max | integer checks per second
backend = "auto"                # auto | screen | screen-shm | window-buffer | damage

[color]
model = "oklch"                 # oklch | hsv | rgb
exit_margin = 1.5               # wide range = tight range × 1.5

[[color.include]]
hex = "#3BD16F"
tol_l = 0.10                    # lightness ± (the UI shows 10)
tol_c = 0.05                    # colorfulness ± (the UI shows 5)
tol_h = 7.0                     # hue ± degrees (oklch and hsv)
# tol_s = 0.15, tol_v = 0.15    # hsv: saturation and value ± (0–1)
# tol_rgb = 12                  # rgb: each channel ± (0–255; 0 = exact)
# a missing tolerance takes the Normal preset's value (Tight for exclude colors), ADR 0005

[[color.exclude]]
hex = "#6BD13B"
tol_l = 0.05
tol_c = 0.03
tol_h = 4.0

[trigger]
confirm_samples = 1
confirm_min_ms = 0
rearm_absent_ms = 40            # "Gone after"
rearm_absent_samples = 2
min_fire_interval_ms = 25       # "Min gap"
max_clicks_per_sec = 15         # "Safety stop"
fire_if_present_on_start = false

[action]
kind = "mouse"                  # mouse | key
button = "left"                 # left | middle | right | back | forward
# key = { keycode = 26, name = "e" }   # for kind = "key"; the keycode comes from the key picker
modifiers = []                  # e.g. ["shift"]
hold_ms = 10
click_at = "pointer"            # pointer | watched-spot

[guards]
only_in_window = true           # "Only click in this window"
pointer_over_window = false
stop_when_locked = true
```

```toml
# ~/.config/glint/settings.toml
schema_version = 1
active_profile = "Sober — green flash"
close_policy = "ask"            # ask | keep-running | quit
reacquire_window = true

[hotkeys]
start_stop = "F7"               # e.g. "Ctrl+Shift+F9"; "" leaves it unbound
emergency_stop = "F8"           # always bound
sample_center = "F6"            # "" leaves it unbound
mode = "toggle"                 # toggle | hold
delivery = "listen"             # listen (XI2 raw; the app also gets the key) | capture (XGrabKey)

[engine]
realtime = "auto"               # off | auto | required
cpu = "none"                    # none | auto | core index
log_level = "info"

[ui]
more_options_open = false
```

---

## Appendix C. X11 debugging toolkit

| Command | What it tells you |
|---|---|
| `xdpyinfo -ext all` | Extensions and versions: XTEST, MIT-SHM, DAMAGE, Composite, RANDR, XInputExtension |
| `xrandr --listmonitors` | Monitor rectangles and which one is primary |
| `xwininfo`, then click a window | Window ID, geometry, depth |
| `xprop`, then click a window | `WM_CLASS`, `_NET_WM_PID`, `WM_STATE` |
| `xprop -root -spy _NET_ACTIVE_WINDOW` | Focus changes as they happen |
| `xev -id <window-id>` | Watch Glint's clicks arrive in a window |
| `xinput list` | Find the "Virtual core XTEST" devices |
| `xinput test-xi2 --root` | Raw input events, exactly as the hotkey code sees them |
| `xset q` | Key auto-repeat delay and rate |
| `Xephyr :2 -screen 1280x720 &` | A nested X server for safe experiments (`DISPLAY=:2 …`) |
| `xvfb-run -a -s "-screen 0 1280x720x24" <command>` | A headless X server for tests |
| `picom --backend xrender &` and `pkill picom` | Switch a compositor on and off while testing |
| `xtrace -- glintd` | Every X request and reply, to count round trips (package `xtrace`) |
| `ps -L -o tid,cls,rtprio,comm -p "$(pidof glintd)"` | Which engine thread got real-time priority |
| `perf top -p "$(pidof glintd)"` or `cargo flamegraph` | Where CPU time goes |
| `socat - UNIX-CONNECT:"$XDG_RUNTIME_DIR/glint/glintd-0.sock"` | Talk to the engine by hand |

---

## Appendix D. References

- `x11rb`: <https://docs.rs/x11rb> · <https://github.com/psychon/x11rb>
- X protocol extension specifications (XTEST, DAMAGE, Composite, RandR, MIT-SHM, XInput 2), including the XInput 2.1 change that delivers raw events regardless of grabs (`XI2proto.txt`): <https://gitlab.freedesktop.org/xorg/proto/xorgproto> · <https://www.x.org/releases/current/doc/>
- EWMH (`_NET_ACTIVE_WINDOW`, `_NET_WM_CM_Sn`, `_NET_CLIENT_LIST`): <https://specifications.freedesktop.org/wm-spec/latest/>
- ICCCM (`WM_STATE`, client windows): <https://www.x.org/releases/current/doc/xorg-docs/icccm/icccm.html>
- OKLab: <https://bottosson.github.io/posts/oklab/>
- Slint documentation: <https://docs.slint.dev> · Slint 1.17 (tray icon, tooltips): <https://slint.dev/blog/slint-1.17-released> · Slint 1.18: <https://slint.dev/blog/slint-1.18-released> · Slint licensing: <https://github.com/slint-ui/slint/blob/master/LICENSE.md>
- rtkit D-Bus interface `org.freedesktop.RealtimeKit1` (`MakeThreadRealtime`, `MaxRealtimePriority`, `RTTimeUSecMax`)
- Linux man pages: <https://man7.org/linux/man-pages/> — `sched(7)`, `prctl(2)` (`PR_SET_TIMERSLACK`), `clock_nanosleep(2)`, `mlockall(2)`, `getrlimit(2)` (`RLIMIT_RTTIME`), `unix(7)` (`SO_PEERCRED`)