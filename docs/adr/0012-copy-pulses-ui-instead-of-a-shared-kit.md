# 0012. Copy Pulse's Slint files instead of sharing a kit

- **Status:** Accepted (owner decision)
- **Date:** 2026-11-13
- **Plan sections:** §2, §12.1, §12.2, §12.8, §13, §14, §19, §20, §21, Appendix A.7

## Context

§12.1 moved Pulse's theme and components into a shared `slint-kit` crate used by both apps, so they'd keep looking
identical as either evolved. That needs:
- a third repository (or a submodule, or one combined repository);
- changes to Pulse's repository;
- a pixel comparison of Pulse before and after.

The owner's view: Glint is a completely separate project from Pulse; it only borrows Pulse's look.

## Decision

- **Copy, once.** Pulse's theme and components are copied into `glint-ui-view/ui/pulse/` at the start of Phase 4.
  Glint's own pieces (swatch, shade strip, magnifier) go next to them, in the same style.
- **Pulse stays untouched.** Nothing in Pulse's repository changes.
- **Origin noted.** The copied files keep a header naming their origin (Pulse, MIT). MIT code may be included in a
  GPL project.
- **Same Slint.** Glint keeps Pulse's Slint version and features (`=1.18.1`), so the copied files compile the same.

## Consequences

- No kit repository, no change to Pulse, and no pixel comparison of Pulse.
- If Pulse is restyled later, Glint doesn't follow automatically: the change has to be copied over on request.
- §21.3 (where the kit lives) is closed.
