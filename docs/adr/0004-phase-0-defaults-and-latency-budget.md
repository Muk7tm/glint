# 0004. Phase 0: defaults confirmed, latency and server-tax expectations corrected

- **Status:** Accepted. Decision 3 (how the v1.0 targets treat compositors and X server stalls) was made by
  the project owner on 2026-11-13.
- **Date:** 2026-11-13
- **Plan sections:** §3, §5.6, §16, §19, §20

## Context

§3 expected a 3×3 `GetImage` round trip of ~0.03–0.3 ms and set engine-side targets of p99 ≤ 2 ms with a
10 ms hard ceiling. §5.6 targeted a server tax of under 0.2 ms at p99 to other clients at Balanced. Phase 0
measured the reference machine (NVIDIA 620.70.06, Xorg 21.1.26, LXQt; `docs/MEASUREMENTS.md`):

- **Round trip.** p50 is 16–20 µs, an order of magnitude better than expected. The tail is set by the X
  server being busy with other clients, about 10% of the time with no compositor. Paced at 2,000/s, p99 is
  1.1–1.16 ms. With picom it is 2.7 ms (xrender) or 3.1–3.2 ms (glx), for both root and window-buffer reads.
- **Server tax.** Balanced adds about +0.45 ms at p99 to other clients' round trips (median 1,103 vs 648 µs;
  seven interleaved runs), and Eco about +0.25 ms. The same number of trivial requests adds nothing. The cost
  is GetImage making the NVIDIA server wait for GPU work queued by other clients.
- **Server stalls.** 10–18 ms stalls happen about once a minute in every configuration, including runs with
  no Glint load at all.
- **MIT-SHM** gives no gain for a 3×3 patch.

## Decision

1. **Defaults confirmed:** Balanced (2,000 checks/s) and the `screen` backend; the backend auto rule is unchanged.
   - Paced response is dominated by server busy time, not by the pacing interval, so Max buys almost nothing.
   - Eco would add up to 1.5 ms of waiting to save about 0.2 ms of server tax.
2. §3, §5.6 and §20 record the measured numbers.
3. **How the v1.0 targets are judged** (owner's choice, option A; the owner plays with the compositor off):
   - **Timing** (p99 ≤ 2 ms, never above 10 ms) is judged with no compositor running.
   - `glint-lab` runs a probe client alongside every timing run. Trials that coincide with an X server stall (the
     probe sees the server stop answering) are listed separately with the stall, and the timing targets apply to
     the rest.
   - **Correctness** (zero extra clicks, zero missed appearances) applies to every trial, stalls included. The only
     miss a stall can cause is one the engine itself reports as possible (ADR 0007); the lab lists those
     separately too.
   - Runs with a compositor are measured and documented in TUNING, not pass/fail.

## Consequences

- The expected engine-side response without a compositor on the reference machine is p50 ≈ 0.27 ms and
  p99 ≈ 1.6 ms, which meets the target.
- With a compositor, p99 ≤ 2 ms is not reachable on this machine: the extra time is server-side compositing
  and GPU syncs, which Glint cannot shorten.
- A check that lands in a 10–18 ms server stall exceeds the 10 ms ceiling whatever Glint does; decision 3 keeps
  those trials visible without failing the timing targets for them. §16 (the lab report), §19 (Phase 6 exit and
  the definition of done) and §20 say so.
- The DAMAGE backend (Phase 6) is the main lever for the server tax, because it cuts the number of GetImages.
  That measurement decides §21.6.
