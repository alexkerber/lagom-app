# Performance

Goal: the most optimised email client there is. Every number below is a budget that CI measures. A change that breaks a budget does not merge.

## Budgets

The targets are ours, set 2026-11-06. None is measured yet.

| What                                    | Budget                                | Measured how                                  |
| --------------------------------------- | ------------------------------------- | --------------------------------------------- |
| Cold start to first frame               | < 200 ms                              | Timestamp at `main` to first presented frame. |
| Key press to updated frame (J, K, E, L) | < 8 ms (one frame at 120 Hz)          | Frame timing in a scripted run.               |
| Open a message from the local store     | < 16 ms                               | Same.                                         |
| Local search, 100 000 messages          | < 50 ms to first results              | Benchmark on a fixed test mailbox.            |
| Memory, one account, 100 000 messages   | < 100 MB resident                     | `footprint` on macOS, equivalents elsewhere.  |
| Idle CPU                                | ~0 %, no timers while nothing changes | Activity Monitor or `powermetrics`.           |
| Release binary                          | < 30 MB                               | File size in CI.                              |

## Rules

1. Local first. The UI reads only from the local store. Network never blocks a frame.
2. Do the work once. Parse a message once, index it once, store the result.
3. Show only what is on screen. Long lists render visible rows only (GPUI `uniform_list`).
4. Idle means idle. Push sync (IMAP IDLE, Gmail and Microsoft push), no polling loops.
5. Measure before optimising, and keep the measurement in CI.
6. A new dependency must earn its place in startup time, memory and binary size.

## Baseline

Measured 2026-11-06 on macOS 27.2, Apple silicon, release build of the static main window (commit after `d21c5ce`). One sample, taken about one second after launch.

| What | Value | Budget |
|------|-------|--------|
| Release binary | 6.0 MB | < 30 MB |
| Resident memory (`ps` RSS) | 107 MB | < 100 MB |
| Physical footprint (`footprint`) | 326 MB | not set; includes GPU memory |
| CPU in the sample | 23 % | startup, not idle |

Not measured yet: start time, frame times, idle CPU.
