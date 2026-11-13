# 0008. Profile and settings files: defaults, strictness, limits and hotkey syntax

- **Status:** Accepted
- **Date:** 2026-11-13
- **Plan sections:** §11, Appendix B

## Context

§11 and Appendix B define the two files mostly by example. Implementing the types, validation and migrations
needed answers the plan doesn't give:

- **Fields.** Which are required? What happens to missing or unknown fields?
- **Versions.** What happens when `schema_version` is missing, newer than this build, or older?
- **Missing limits.** §11 sets none for the profile name, the number of colors, the largest tolerances, or the
  offset before any geometry is known.
- **Hotkeys.**
  - How a hotkey is written, and whether one can be left unbound.
  - Which pairs count as "the same hotkey".
  - How "action key ≠ any hotkey" compares the action key, which is stored as a keycode plus a name, with hotkeys,
    which are names.
- **New profiles.** Their default watch mode.

## Decision

- **Defaults everywhere, typos are errors.**
  - Every section and field has a default: Appendix B's values. A new profile watches the screen center and has
    no color yet. So a hand-written file needs only `schema_version` and `name`.
  - Unknown fields are errors, so a misspelled field can't silently fall back to its default.
  - Fields of another color model or pattern are kept and ignored (ADRs 0005 and 0006), but must still hold
    legal values.
- **Versions.**
  - Files must carry `schema_version`.
  - An older schema is migrated one version at a time; the engine's store keeps a `.bak` of the original and
    saves the new version.
  - A newer schema is refused with "update Glint", never guessed at.
  - A file already at the current schema is parsed straight from its text, so errors keep their line and column.
  - Both schemas are 1 and have no migrations yet. A test drives the mechanism with a made-up two-step chain.
- **Limits** added to §11:
  - a name of 1–64 characters with no control characters;
  - at most 16 include and 32 exclude colors (each one adds work to every table build);
  - tolerances above 0 and at most ΔL 1, ΔC 0.5, Δh 180°, ΔS and ΔV 1 (beyond these a range already covers the
    whole axis);
  - an offset within ±16,384 px until the geometry is known. The geometric rule "the offset keeps the whole
    pattern on the monitor or inside the window" runs at Start (`validate::placement`).
- **Hotkeys.**
  - **Syntax.** Written like `F7` or `Ctrl+Shift+F9`: modifiers `Ctrl` (or `Control`), `Alt`, `Shift` and
    `Super` in any case, then one X key name made of letters, digits and underscores.
  - **Unbinding.** `""` leaves start/stop or sample unbound. The emergency stop must always be bound.
  - **Duplicates.** Two bindings collide when they have the same key, ignoring case, and the same modifiers.
  - **Blocked combinations.** Ctrl+Alt+F1…F12 and Ctrl+Alt+Backspace are blocked whatever other modifiers are
    added.
  - The engine resolves names to keycodes with the live keymap and rejects names it doesn't know (Phase 2).
- **Action key against hotkeys.** The action key's name is compared with each hotkey's key name, ignoring case
  and modifiers. This is the stricter reading. The engine also compares keycodes once it has resolved the
  hotkeys.
- **Warnings** (shown, but the change is applied):
  - "Gone after" under 15 ms;
  - the Max speed;
  - a gray include color;
  - a hold at least as long as the minimum gap, which then sets the real gap (ADR 0007);
  - an exclusion that cuts into an include range, from the lookup table's statistics.
- **Valid but not startable.** A profile with no include color ("Pick a color to start") or in window mode
  without a window ("Pick a window to start") is valid, but Start refuses it.

## Consequences

- The tests read Appendix B's two examples straight from `docs/PLAN.md`, so the plan's examples can't drift away
  from what the code accepts.
- The protocol's `set_profile` and `set_settings` carry the same structures as JSON (round-trip tests), with the
  same defaults and the same strictness.
