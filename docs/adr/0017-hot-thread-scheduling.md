# 0017. The hot thread's scheduling: rtkit only, a safe memory lock, and the screen lock by signal

- **Status:** Accepted
- **Date:** 2026-11-13
- **Plan sections:** §5.6, §8, §10, §19 (Phase 6)

## Context

§5.6 asks for:
- real-time priority through rtkit, or directly with `RLIMIT_RTPRIO`;
- normal priority under Max;
- 1 ns timer slack;
- prefaulting, and `mlockall(MCL_CURRENT | MCL_FUTURE)` "best-effort";
- optional CPU pinning.

ADR 0014 moved the D-Bus screen lock here. What the work found:

1. **No safe `sched_setscheduler`.** rustix 1.1.6 and nix 0.31.3 don't wrap it, and `libc` needs `unsafe`, which
   §13 allows only in `shm.rs`. rtkit sets the policy for us. Its `MakeThreadHighPriority` also switches a
   thread back to `SCHED_OTHER`: the scheduling test sees policy 0 after a switch to Max.
2. **rtkit requires `RLIMIT_RTTIME`**, at most its `RTTimeUSecMax` (200 ms on the owner's desktop). A real-time
   thread that runs that long without blocking is killed.
3. **`MCL_FUTURE` with a finite memory-lock limit is a trap.** The owner's limit is 8 MiB, and the engine
   needs more. If `mlockall` succeeds, a later allocation beyond the limit fails, and Rust aborts on that.
4. **Prefaulting is already done.** `Lut::build` writes every word, so all its pages are resident before the
   hot thread reads them.
5. **Phase 0 measured nothing on pinning.**
6. **The owner's LXQt has no D-Bus screen saver service.** KDE, GNOME, MATE and Cinnamon each announce a lock
   with an `ActiveChanged(bool)` signal.
7. **Test engines asking rtkit is unsafe for the test run.** Dozens of them could hit rtkit's per-user limits,
   or the owner's real engine could be refused, and real-time hot threads could starve the sandboxes' X
   servers. A settings file without `schema_version` is rejected, which silently put test engines back to
   `auto`. This was found by the new guard below.

## Decision

- **rtkit only.**
  - With `auto` or `required`, and a speed other than Max, the controller asks rtkit for `SCHED_RR` at
    priority 10, capped at rtkit's maximum.
  - Under Max, or with `off`, `MakeThreadHighPriority(tid, 0)` puts the thread back.
  - There is no direct `RLIMIT_RTPRIO` path. rtkit runs on every desktop with PipeWire or PulseAudio.
- **`RLIMIT_RTTIME`:** hard limit `RTTimeUSecMax`, soft limit half of it. The engine catches the soft limit's
  `SIGXCPU`, drops the hot thread to normal priority, and warns "Real-time priority is off: the engine ran too
  long without a pause." The thread stays normal until the engine restarts.
- **`required`** refuses Start when real-time wasn't granted ("Real-time priority is required, but …"). Max
  is exempt, because it never runs real-time.
- **The memory lock** happens only when `RLIMIT_MEMLOCK` is unlimited. Otherwise the engine reports "not
  locked: RLIMIT_MEMLOCK is 8192 KiB".
- **Timer slack** is set to 1 ns by the hot thread itself. Linux shows no per-thread value in `/proc`, so a
  unit test checks the call.
- **CPU:** `none` and `auto` run anywhere, as the plan says for `auto` without a measurement. A core number pins
  the hot thread there, and a missing core is reported, not fatal.
- **Reporting:**
  - `state` gains `realtime` (for example "rtkit (rr 10)") and `cpu` ("any core", "core 3").
  - `hello_ack.session` reports the real priority, and `memory`.
  - `glint-ctl status` prints `real-time: … · runs on …`.
  - The UI's notice comes with the ENGINE card (Phase 6, item 3).
- **The screen lock over D-Bus** listens for `ActiveChanged` signals from `org.freedesktop.ScreenSaver`,
  `org.gnome.ScreenSaver`, `org.mate.ScreenSaver` and `org.cinnamon.ScreenSaver` on the session bus. It doesn't
  poll `GetActive`. Without a session bus it does nothing. Tested on a private `dbus-daemon`.
- **Tests:**
  - Test and lab sandboxes write `schema_version = 1` and `realtime = "off"`; a guard asserts "off".
  - One test runs with `auto` and `cpu = 0`. Where rtkit answers, it checks the kernel's view of the hot thread
    (`SCHED_RR` 10, core 0, back to `SCHED_OTHER` under Max, the other threads normal). In CI it checks the
    fallback.
  - `cargo test` now limits engine sandboxes the way the lab tests are limited (`tests/common`), after one load
    flake with 11 at once.

## Consequences

- The owner's desktop gets real-time priority (rtkit is running).
- A desktop without rtkit runs at normal priority and says why. One that has only `limits.d` configured gets
  no real-time, a narrow loss. Revisit with an ADR if a safe wrapper appears.
- The memory isn't locked on typical desktops. The LUT is read 2,000 times a second, so it stays in memory
  unless the machine is out of memory.
