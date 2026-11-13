# Architecture decision records

One short file per decision that changes or settles something in `docs/PLAN.md`. Every ADR that changes the
plan also updates the affected plan section in the same commit.

| # | Title | Status | Plan sections |
|---|---|---|---|
| [0001](0001-dependency-versions.md) | Dependency versions differ from §14 | Accepted | §14 |

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
