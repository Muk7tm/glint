# Glint: working rules

**Read `docs/PROGRESS.md` first.** It says which phase is active, what is done, and what is blocked.
Decisions that changed the plan are indexed in `docs/adr/README.md`.

Glint is a low-latency, color-triggered clicker for Linux X11: a Rust engine (`glintd`) plus a separate
Slint UI (`glint`). The specification is **`docs/PLAN.md`**. It is the source of truth: follow it, and when
it is wrong, change it openly (see Deviations), never silently.

## Non-negotiables

- Rules R1–R7 (PLAN §1) and the six trigger invariants (PLAN §5.5), each backed by tests. Never trade them
  for speed.
- Speed on the rising edge, safety on the falling edge. Fire on the first Enter check; all anti-double-click
  logic lives in the re-arm path.
- Engine and UI are separate processes. The UI never injects input; the engine never waits on the UI.
- The hot loop has no locks, no logging, no allocation, and no I/O beyond X traffic and sleeping.
- Watching never starts on its own.
- If the color is already present on any (re)start, latch instead of firing.
- Release held input on every stop path, and do a defensive release at startup.
- Only one engine runs per display.
- Stay inside the plan's scope: no anti-detection or "humanized" input, no Wayland-native code in v1,
  nothing that reads other processes' memory.
- `#![forbid(unsafe_code)]` in core, ipc and ui (and every other crate unless the plan or an ADR allows
  otherwise). `unsafe` only where PLAN §13 allows it (`glint-x11/src/capture/shm.rs`), with SAFETY comments.
  Slint's generated code needs `allow(unsafe_code)`, so it lives alone in `glint-ui-view` under `deny`
  (ADR 0002); every hand-written UI line stays in `glint-ui` under `forbid`.
- The hot connections A and B use the allocation-free raw protocol path, not x11rb (ADR 0003).
- Before every commit, all green:

  ```sh
  cargo fmt --all --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```

  When `spikes/` changed, run the same three inside `spikes/` (it is its own workspace).

## Workflow

- Work through the phases in PLAN §19 strictly in order. Phase 0 measurements can change defaults.
- Phase 0 spikes live in `spikes/` as small throwaway crates (own workspace, excluded from the main one).
  Real numbers go in `docs/MEASUREMENTS.md`, with the environment: X server version, GPU/driver, compositor,
  refresh rates, `XDG_SESSION_TYPE`.
- Tick a checklist item in `docs/PROGRESS.md` only when it is implemented and its tests pass. Commit after
  each item with a clear message.
- When a phase meets its exit criteria, stop and report: what was built, test and measurement results,
  deviations from the plan, and the plan for the next phase. Then wait for the user's go-ahead.
- Keep reports short and concrete.

## Verify, don't guess

Appendix A of the plan has sketches, not drop-in code. Check every API against the real docs or the crate
source for the versions in use (x11rb 0.13.2 and Slint 1.18.1; sources are in `~/.cargo/registry/src/`).
Never invent functions, features or flags. If something can't be verified, say so.

## Deviations

When the plan proves wrong (an API differs, a measurement contradicts it, or a better approach appears):

1. Write a short ADR in `docs/adr/` (template and index in `docs/adr/README.md`).
2. Update the affected section of `docs/PLAN.md`.
3. Say so in the next report.

## Safety on the user's machine

- Until Phase 2's exit criteria pass, anything that injects input runs only inside Xephyr
  (`Xephyr :2 -screen 1280x720`) or Xvfb. Read-only spikes may use the real display (`:0`).
- Ask before the first input-injecting run on the real desktop. The F8 emergency stop and
  `glint-ctl release-all` must work first.
- No sudo and no system config changes. Give the user the command (packages, limits.d, rtkit) to run.

## Pulse

The user's Rust + Slint app Pulse lives at `/home/muk7tm/Documents/VSCode/Tools/Pulse`. Its look is Glint's
UI spec (PLAN §12).

- Read it freely. Glint copies Pulse's theme and components into `glint-ui-view/ui/pulse/` (ADR 0012); there is
  no shared kit, and Glint is a separate project.
- Never change anything in Pulse's repository.
- Keep the copied files' origin note (Pulse, MIT), and use Pulse's Slint version and features (`=1.18.1`).

## Defaults and questions

Use the plan's defaults. Ask only about choices the plan leaves open (PLAN §21: name, license,
hotkeys), and only when they are actually reached.

## Environment notes

- Rust is Arch's system package (1.99.1), with no rustup: `rust-toolchain.toml` is inert here and the musl
  target is not installed (needed only for Phase 7).
- Session: Xorg 21.1.26, LXQt + Openbox, NVIDIA proprietary driver, one 1920×1080 @ 60 Hz monitor, no
  compositor by default (picom is installed). Details in `docs/MEASUREMENTS.md`.
