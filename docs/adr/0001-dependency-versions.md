# 0001. Dependency versions differ from §14

- **Status:** Accepted
- **Date:** 2026-11-13
- **Plan sections:** §14

## Context

§14 lists major versions that were current when the plan was written. On 2026-11-13, crates.io and Pulse's
`Cargo.lock` show:

| Crate | §14 | Current | Pulse |
|---|---|---|---|
| x11rb | 0.13 | 0.14.0 | 0.14.0 (its own use; Slint's winit pulls 0.13.2) |
| toml | 0.8 | 1.2.1 | 1.1 |
| rtrb | 0.3 | 0.4.1 | — |
| triple_buffer | 8 | 9.0.1 | — |
| signal-hook | 0.3 | 0.4.6 | 0.4 |
| criterion | 0.5 | 0.8.2 | — |
| slint / slint-build | 1.18 (default features) | 1.18.1 | `1.18.1`, default features off |

The x11rb 0.13.2 → 0.14.0 source diff touches only `rust_connection/stream.rs` (0.14 no longer tries the
abstract Unix socket before the filesystem socket) and `xcb_ffi`, plus an MSRV bump from 1.64 to 1.68. The
protocol API Glint uses is identical.

§5.4 also asks for a cross-check of the OKLab test vectors against the `palette` crate, which §14 does not list.

## Decision

- **x11rb stays on 0.13.2**, as instructed for this project. The API is the same as 0.14, and Slint's winit
  backend already uses 0.13.2, so a combined build carries one copy. `x11rb-protocol` 0.13.2 is listed
  explicitly for its sans-I/O pieces (connection setup, display parsing, Xauthority).
- **Slint is pinned to `=1.18.1` with Pulse's exact feature set** (`std`, `compat-1-18`, `backend-winit-x11`,
  `renderer-femtovg`, `renderer-software`, `accessibility`), so the shared kit compiles identically in both
  apps (§12.1). Pulse and Glint bump together.
- Use the current majors: `toml = "1"`, `rtrb = "0.4"`, `triple_buffer = "9"`, `signal-hook = "0.4"`,
  `criterion = "0.8"`.
- Add `palette = "0.7"` as a dev-dependency of `glint-core` for the test-vector cross-check.
- Each crate's API is checked against the resolved version's source when Glint first uses it.

## Consequences

No behavior in the plan changes. Appendix A sketches that touch these crates are verified against the new
versions when they are implemented, as they would be anyway.
