# 0002. Slint's generated code lives in its own crate so the UI can forbid unsafe

- **Status:** Accepted
- **Date:** 2026-11-13
- **Plan sections:** §12.1, §12.8, §13, Appendix A.6

## Context

The UI crate must carry `#![forbid(unsafe_code)]` (non-negotiable; §13). Phase 0's `spike-ui` showed that
Slint 1.18.1's generated Rust code expands `ItemTreeVTable_static!` (from the `vtable` crate that Slint
re-exports), which carries `#[allow(unsafe_code)]`. Under a crate-level `forbid`, that `allow` is a hard error:

```text
error[E0453]: allow(unsafe_code) incompatible with previous forbid
```

`forbid` cannot be relaxed for a single module, so `slint::include_modules!()` and `#![forbid(unsafe_code)]`
cannot share a crate. Slint emits no `unsafe` of its own unless `SLINT_ASSET_SECTION` is set at build time;
the only source is that macro.

## Decision

Split the UI in two crates:

- **`glint-ui-view`** (library) owns `build.rs`, the `ui/` Slint files and the generated code. It is
  `#![deny(unsafe_code)]`, which the generated `allow` may override, and it contains no hand-written logic beyond
  re-exporting the generated components.
- **`glint-ui`** (binary `glint`) holds every hand-written line of UI code under `#![forbid(unsafe_code)]` and
  uses the generated types from `glint-ui-view`.

`spikes/spike-ui` (forbid) and `spikes/spike-ui-view` (deny) prove that this compiles and runs.

## Consequences

- One more crate. `build.rs` and `ui/` move from `glint-ui` to `glint-ui-view`.
- The only `allow(unsafe_code)` anywhere in the UI is inside Slint's generated code.
- Generated code is also outside `glint-ui`'s clippy policy, so no `#[allow(clippy::all)]` wrapper is needed
  next to hand-written code.
