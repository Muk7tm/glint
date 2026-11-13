# Architecture decision records

One short file per decision that changes or settles something in `docs/PLAN.md`. Every ADR that changes the
plan also updates the affected plan section in the same commit.

| # | Title | Status | Plan sections |
| --- | --- | --- | --- |
| [0001](0001-dependency-versions.md) | Dependency versions differ from §14 | Accepted | §14 |
| [0002](0002-slint-generated-code-and-forbid-unsafe.md) | Slint's generated code lives in its own crate so the UI can forbid unsafe | Accepted | §12.1, §12.8, §13, A.6 |
| [0003](0003-raw-protocol-hot-path.md) | Allocation-free raw X protocol path for the hot connections | Accepted | §4.2, §5.6, §13, A.3–A.4 |
| [0004](0004-phase-0-defaults-and-latency-budget.md) | Phase 0: defaults confirmed, latency and server-tax expectations corrected; how v1.0 judges stalls and compositors | Accepted (owner decision) | §3, §5.6, §16, §19, §20 |
| [0005](0005-color-rule-details.md) | Color rules: HSV and RGB tolerances, exclusion defaults, and a gray-pixel guard | Accepted | §5.4, §11, Appendix B |
| [0006](0006-sample-pattern-details.md) | Sample patterns: k_stay = ⌈k_enter / 2⌉, plus and custom fields, the anchor pixel | Accepted | §5.1, §5.3, Appendix B |
| [0007](0007-trigger-restarts-gaps-and-observed-absence.md) | Trigger: every restart latches, the hold sets a minimum gap, only watched absence counts, possible misses are reported | Accepted (point 5 revised) | §1, §5.5, §7, §16, §17, A.1–A.2 |
| [0008](0008-profile-and-settings-format.md) | Profile and settings files: defaults, strictness, limits and hotkey syntax | Accepted | §11, Appendix B |
| [0009](0009-engine-control-details.md) | Engine control: armed and gate bits, the display lock, monitor fallback, releasing everything, UI disconnects | Accepted | §4.2, §4.3, §4.4, §5.1, §10 |
| [0010](0010-lab-trace-stream-and-judging.md) | The lab: a trace stream from the engine, judging with time bounds, and what Phase 3 covers | Accepted | §10, §16, §19 |
| [0011](0011-a-check-reads-the-screen-between-request-and-reply.md) | A check reads the screen somewhere between its request and its reply: the trigger uses both times | Accepted | §5.5, A.1–A.2 |
| [0012](0012-copy-pulses-ui-instead-of-a-shared-kit.md) | Copy Pulse's Slint files instead of sharing a kit | Accepted (owner decision) | §2, §12, §13, §14, §19–§21, A.7 |
| [0013](0013-hold-to-watch-typed-colors-and-side-button-hotkeys.md) | Hold to watch, typed color codes, and mouse side buttons as hotkeys | Accepted (owner requests) | §9, §12.3, §19 |
| [0014](0014-window-buffer-gap-and-lock-detection.md) | The window-buffer backend: its check gap and its self-check; D-Bus lock detection moves to Phase 6 | Accepted | §5.2, §8, §12.3, §19 |

## Template

```markdown
# NNNN. Title

- **Status:** Proposed | Accepted | Superseded by NNNN
- **Date:** YYYY-MM-DD
- **Plan sections:** §x.y

## Context
What the plan says, and what turned out to be wrong, missing or better (with evidence: source lines,
measurements, test output).

## Decision
What we do instead.

## Consequences
What changes, what it costs, and what it rules out.
```
