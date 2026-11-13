# Architecture decision records

One short file per decision that changes or settles something in `docs/PLAN.md`. Every ADR that changes the
plan also updates the affected plan section in the same commit.

| # | Title | Status | Plan sections |
| --- | --- | --- | --- |
| [0001](0001-dependency-versions.md) | Dependency versions differ from §14 | Accepted | §14 |
| [0003](0003-raw-protocol-hot-path.md) | Allocation-free raw X protocol path for the hot connections | Accepted | §4.2, §5.6, §13, A.3–A.4 |
| [0004](0004-phase-0-defaults-and-latency-budget.md) | Phase 0: defaults confirmed, latency and server-tax expectations corrected | Accepted; v1.0 target treatment open | §3, §5.6, §20 |

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
