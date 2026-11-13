# 0003. Allocation-free raw X protocol path for the hot connections

- **Status:** Accepted
- **Date:** 2026-11-13
- **Plan sections:** §4.2, §5.6, §13, Appendix A.3–A.4

## Context

The hot loop must not lock or allocate (non-negotiable; §4.2, §5.6). The plan assumed the only steady-state
allocation would be the reply buffer of plain `GetImage`, and that MIT-SHM removes even that.

x11rb 0.13.2's source shows otherwise:
- Every request allocates its bytes: `GetImageRequest::serialize` and `xtest::FakeInputRequest::serialize`
  both build a `vec!`.
- The packet reader allocates `vec![0; len]` for every reply, and `GetImageReply` copies the data into
  another `Vec<u8>`.
- `RustConnection` takes several internal `Mutex`es on every call: `inner`, `packet_reader`, and others.

Measured in Phase 0 (`spike-capture`, `docs/MEASUREMENTS.md` §1), with a counting global allocator:

| Path | Heap allocations per check | Round trip p50 / p99 |
|---|---|---|
| x11rb `get_image().reply()` | 5 | 16.4 / 32.9 µs |
| x11rb `shm_get_image().reply()` | 3 | 16.0 / 31.4 µs |
| raw client (fixed buffers, blocking socket) | 0 | 16.6 / 33.3 µs |

`assert_no_alloc` would therefore trip on the x11rb path. In the planned static musl build, malloc also takes
a global lock shared with the controller, telemetry and rayon threads, which is a real latency hazard and
not just a rule violation.

## Decision

Connections A (capture) and B (input) use a small raw X protocol client in `glint-x11` (`wire.rs`):

- Connection setup, display parsing and Xauthority come from x11rb-protocol's sans-I/O helpers
  (`parse_display`, `connect::Connect::new`, `into_setup`). Those allocate once, at startup.
- After setup, every request is a preformatted fixed-size buffer written with one `write`. Replies, errors
  and events are parsed in place from a fixed read buffer on a blocking Unix stream.
- Events that arrive unselected (for example `MappingNotify`) and asynchronous errors are drained and
  reported through the event ring; they never allocate.
- Extension major opcodes and event bases (XTEST, MIT-SHM, DAMAGE) come from connection C, which uses x11rb.
  X resources are server-wide, so connection A can use an SHM segment attached on C.
- Connections C and D, and everything outside the hot thread, keep using x11rb.
- Debug builds wrap the hot loop body in `assert_no_alloc`, so the rule is enforced by tests.

Everything stays safe Rust; no new `unsafe`.

## Consequences

- Glint owns a few hundred lines of protocol code: GetImage, ShmGetImage, XTest FakeInput and GrabControl,
  and later DamageNotify parsing. They are covered by request-encoding and packet-parsing unit tests plus
  Xvfb integration tests.
- The hot connections support local Unix-socket displays only. Remote X is unsupported anyway (§8).
- The `spike-capture` raw client is the reference implementation.
