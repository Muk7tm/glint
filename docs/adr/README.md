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
| [0015](0015-loose-plus-shades.md) | *Loose+*: a fourth shades option that takes much lighter and darker shades | Accepted (owner request) | §5.4, §12.3 |
| [0016](0016-mit-license-and-about-dialog.md) | MIT license, with Pulse's About dialog showing AboutSlint | Accepted (owner decision) | §2, §12.3, §13, §14, §16, §18, §21 |
| [0017](0017-hot-thread-scheduling.md) | The hot thread's scheduling: rtkit only, a safe memory lock, and the screen lock by signal | Accepted | §5.6, §8, §10, §19 |
| [0018](0018-damage-backend.md) | The `damage` backend: reports on the hot connection, a 15 ms check gap, opt-in only | Accepted | §5.2, §5.5, §19, §21 |
| [0019](0019-more-options-panels.md) | More options: what the plan left open about its three panels | Accepted | §10, §12.4, §12.9, §17, §19 |
| [0020](0020-capture-gives-input-back.md) | Capture mode gives the keyboard or pointer back at once | Accepted | §9, §12.3, §19 |
| [0021](0021-v1-on-the-owners-machine-and-install-sh.md) | Phase 7: v1.0 is checked on the owner's machine, nothing is published, and `install.sh` installs it | Accepted (owner decisions) | §12.4, §13, §17, §18, §19 |
| [0022](0022-packaging.md) | Packaging: a static engine built apart from the window, staged files, an Ubuntu 22.04 build in CI, and the engine copied out of the AppImage | Accepted | §13, §14, §18, §19 |
| [0023](0023-click-if-already-there.md) | *Click if it's already there when I start*: a switch for `fire_if_present_on_start` | Accepted (owner request) | §5.5, §9, §12.3, §16 |
| [0024](0024-sleep-without-spinning.md) | The hot loop sleeps to each check's deadline, without spinning | Accepted | §5.6, §19, A.2 |
| [0025](0025-area-pattern-and-must-last.md) | The Area pattern and *Must last*, from the owner's training recording | Accepted (from the owner's recording and request) | §5.3, §5.5, §12.4, §16 |
| [0026](0026-whole-evidence-in-the-lab.md) | The lab's evidence must be whole: dropped engine records are marked, and the lab reads every connection | Accepted | §10, §16 |
| [0027](0027-two-column-window.md) | The window shows its cards in two columns, and *More options* scrolls | Accepted (owner request) | §12.2, §12.3, §12.4, §12.8 |

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
