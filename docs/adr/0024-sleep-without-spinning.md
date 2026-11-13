# 0024. The hot loop sleeps to each check's deadline, without spinning

- **Status:** Accepted
- **Date:** 2026-11-14
- **Plan sections:** §5.6, §19, A.2

## Context

§5.6 had the hot thread sleep until about 50 µs before each check's deadline, then spin the rest. The code
spun the last 60 µs. The v1.0 definition of done asks for the engine's CPU at Balanced, measured, with a budget
of 10% of one core. Measured on the owner's machine (i5-11400F, kernel 7.2 zen) while their game ran:

- **The engine used 13% of one core** while watching at Balanced, and nothing while stopped. All of it was
  the checking thread (`/proc/<pid>/task/*/stat` over 5 s, a release build on Xvfb).
- **Almost all of that was the spin.** 2,000 checks a second × 60 µs is 12% of a core. Under real-time
  priority, that thread also takes its core away from the game for that time.
- **A benchmark of the same pacing** (1 ns timer slack, 0.5 ms period, 20,000 checks):

  | Spin | Priority | Late p50 | Late p99 | Late p99.9 | Late max | CPU |
  |---|---|---|---|---|---|---|
  | 60 µs | normal | 0.0 µs | 0.1 µs | 39 µs | 195 µs | 11.5% |
  | 60 µs | SCHED_RR 10 | 0.0 µs | 0.1 µs | 1.8 µs | 604 µs | 11.7% |
  | 20 µs | SCHED_RR 10 | 0.0 µs | 0.1 µs | 23 µs | 68 µs | 3.6% |
  | none | normal | 3.9 µs | 15 µs | 54 µs | 267 µs | 0.5% |
  | none | SCHED_RR 10 | 3.5 µs | 8.7 µs | 12 µs | 31 µs | 0.4% |

A check that comes a few microseconds late delays a click by those microseconds. The wait for the next check is
already 250 µs on average at Balanced, and the measured p99 response is 0.72 ms against a 2 ms target.

## Decision

- **The hot thread sleeps until each deadline** and no longer spins: `std::thread::sleep(deadline - now)`,
  with 1 ns timer slack.
- Deadlines stay absolute (`next = previous + period`), so lateness doesn't add up.
- Max still never sleeps.
- **`glint-lab basic` and `soak` report the engine's CPU over the run**, both the whole engine and the
  checking thread (`glintd-hot`).
  - In `soak`, the whole engine also includes the color tables that its profile edits rebuild on every
    core: about 1.3 CPU-seconds per edit.
  - The checking thread's share is the cost of watching.

## Consequences

- Watching at Balanced costs 1.8% of one core (1.6% without real-time), down from 13%. The game gets that
  core back.
- On Xvfb, `basic` with 200 appearances clicked every one: 0.31 ms p50, 0.56 ms p99, the engine at 1.9% of a
  core. The owner's 60-minute soak on the real desktop measures the same with a GPU.
