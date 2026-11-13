# 0004. Phase 0: defaults confirmed, latency and server-tax expectations corrected

- **Status:** Accepted (defaults and recorded numbers). How the v1.0 targets treat compositors and X server
  stalls is an open question for the project owner.
- **Date:** 2026-11-13
- **Plan sections:** §3, §5.6, §20

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
2. §3, §5.6 and §20 record the measured numbers. The targets stay as written for now.

## Consequences

- The expected engine-side response without a compositor on the reference machine is p50 ≈ 0.27 ms and
  p99 ≈ 1.6 ms, which meets the target.
- With a compositor, p99 ≤ 2 ms is not reachable on this machine: the extra time is server-side compositing
  and GPU syncs, which Glint cannot shorten.
- A check that lands in a 10–18 ms server stall exceeds the 10 ms ceiling whatever Glint does. Proposed for the
  owner to decide: `glint-lab` runs a concurrent probe client and reports trials that coincided with a server
  stall separately, and the v1.0 ceiling applies to Glint-attributable time.
- The DAMAGE backend (Phase 6) is the main lever for the server tax, because it cuts the number of GetImages.
  That measurement decides §21.6.
