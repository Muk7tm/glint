# 0026. The lab's evidence must be whole: dropped engine records are marked, and the lab reads every connection

- **Status:** Accepted
- **Date:** 2026-11-14
- **Plan sections:** §10, §16

## Context

The v1.0 definition of done asks for 10,000 trials of every lab scenario on the owner's machine. They ran
overnight on 2026-11-14, twenty scenarios side by side. To leave the desktop alone, they ran at idle priority
(`SCHED_IDLE`), each in its own Xvfb, and the sandbox engines run without real-time priority. At times, builds
and tests at normal priority took the CPU from them completely.

No run sent an extra click. Eight runs failed, and each failure was a gap in the lab's evidence, not a wrong
decision by the engine:

1. **`live-edit`:** its trace connection fell a full outbox behind, and the engine let it go. The judge took
   the cut-off trace for a whole one. This was fixed the same night (commit 47321fe); the rerun passed.
2. **`fade`:** 19 appearances got neither a click nor a reported possible miss. They all came between
   03:50:20 and 03:50:44, while the engine's trace showed it checking. This was the minute a normal-priority
   lab test and a window ran next to the runs, just after they were moved to idle priority.

   The hot thread hands every record to the telemetry thread through a ring of 16,384 entries. A full ring
   drops records, and only a total (`dropped`) counts them. A dropped possible-miss report leaves no trace.
   The checks dropped around it look like blind time to the telemetry thread. The run can't say which records
   were dropped; nothing in the engine said so.
3. **`focus` and `ui-overlap`:** both ended with "engine: unexpected end of file", after 3.1 and 4.7 hours.
   - The engine broadcasts every change of state to every client, and lets a client go when its outbox of 256
     messages is full (§10).
   - The lab read its control connection only while it waited for a reply.
   - These two scenarios go thousands of focus changes or window overlaps without a request, and each one adds
     a broadcast.

   A fake engine that drops slow readers the same way reproduces this in a unit test.
4. **`action-matrix` (105 of 10,005 trials) and the `kill-mid-press` rerun (4 of 10,000):** these trials
   got no press at all.
   - Both scenarios check each trial's exact input, and counted any trial without a press as a problem.
   - A starved engine is blind, and by design it reports a possible miss instead of clicking (ADR 0007).
   - The judge excuses a reported possible miss. These two scenarios never looked at the trace.
5. **`feedback`:** 105 of 10,000 rounds ended without the safety stop the scenario asked of every round. Starved,
   the engine reported possible misses instead of clicking, so one click too many never came due within a
   second. That is the safe outcome, not a failure of the stop.
6. **`fire-on-start` and the first `kill-mid-press`:** these were the lab's own trouble:
   - `fire-on-start`'s trace reader starved, and the run was stopped.
   - `kill-mid-press` gave a restarted engine 5 seconds to listen. Building the color table at idle priority
     took longer. The wait is now 30 seconds (commit 78aaa8d).

## Decision

1. **The hot thread marks what its ring dropped.**
   - When a record doesn't fit, the hot thread remembers the earliest moment a dropped record speaks for.
   - Once two slots are free, it puts `Lost { from, to }` in front of the next record.
   - The telemetry thread passes the marker on as a `lost` trace entry, and doesn't count the gap as blind
     time.

   This adds a message kind, so the protocol stays 1 (§10). The hot loop gains one branch per record, and a
   clock read only after a drop. It still allocates nothing and takes no lock.
2. **The judge treats a gap like a cut-off.** A missing click whose report could have fallen in a `lost`
   entry is untraced. That means unknown: neither missed nor unseen, because blind time and dropped records
   can't be told apart in the gap. The run then fails with that reason, as it does for a cut-off trace.
3. **The lab reads its control connection all the time.** A thread of its own reads it, as the window's client
   does. The thread passes replies on and drops broadcasts.
4. **The per-trial scenarios use the judge's standard.** `action-matrix`, `kill-mid-press` and
   `action-equals-hotkey` collect the trace. A trial without a press is excused only when:
   - the engine reported a possible miss during it; or
   - every showing came and went inside blind time.

   With an incomplete trace, the trial is unknown and the run fails with that reason. Anything else is still a
   problem.
5. **`feedback` ends a round without a stop itself.** It notes how many rounds did, and still checks every stop
   that came and every click's rate. A run where no round stops fails.
6. **The trace reader reads without timeouts.** A thread blocked on the connection can't stop reading halfway
   through a line. The tap ends with a ping on the writing half.
7. **`kill-mid-press` and `feedback` count their notes.** They give one line per action or one summary,
   instead of one line per round, which was 10,000 lines.

## Consequences

- A run can still fail under heavy load, but only for a stated reason: a wrong click, an unexplained miss, or
  incomplete evidence. It never passes on incomplete evidence, and never fails on a properly reported miss.
- The overnight results stand where they passed. The failed scenarios run again with this build. They did,
  with 3912c27, and all six passed 10,000 trials (MEASUREMENTS.md §14).
- Side by side at idle priority is a fair test of correctness, but not of how often Glint clicks.
  `arm-while-present` reported 37% of its appearances as possible misses there. On the owner's desktop, with
  real-time priority, Phase 6's run clicked 10,000 of 10,000.
