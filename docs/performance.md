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
3. Show only what is on screen. Long lists render visible rows plus a small overscan; animated rows use spacers that track their changing heights.
4. Idle means idle. Push sync (IMAP IDLE, Gmail and Microsoft push), no polling loops.
5. Measure before optimising, and keep the measurement in CI.
6. A new dependency must earn its place in startup time, memory and binary size.

## Baseline

Measured 2026-11-06 on macOS 27.2, Apple silicon, release build of the static main window (commit after `d21c5ce`). One sample, taken about one second after launch.

| What                             | Value  | Budget                       |
| -------------------------------- | ------ | ---------------------------- |
| Release binary                   | 6.0 MB | < 30 MB                      |
| Resident memory (`ps` RSS)       | 107 MB | < 100 MB                     |
| Physical footprint (`footprint`) | 326 MB | not set; includes GPU memory |
| CPU in the sample                | 23 %   | startup, not idle            |

Not measured yet: start time, frame times, idle CPU.

## Message list virtualisation

Measured 2026-11-08 with `cargo test -p lagom ten_thousand_message_rows -- --nocapture`,
on macOS in GPUI's headless `TestAppContext`, debug build. The fixture contains
10,000 synthetic messages in a 1440 × 600 test window. One warm-up frame, then
10 forced full-window frames including element construction, layout and paint.
No native app, GPU presentation or real account is involved.

|                               | Before (`e163ce1` + measurement test) | Virtualised |
| ----------------------------- | ------------------------------------: | ----------: |
| Row elements built at the top |                                10,000 |           9 |
| Mean full-frame time          |                           1,093.18 ms |    10.99 ms |

The old list built empty row boxes outside its text-rendering range. The new
list builds two spacers and visible rows with two row heights of overscan on
each side. Off-screen selection does not enlarge that range; glide text copies
are also bounded to it. Sparse animation height changes adjust the offsets.

The test asserts a screenful of elements, checks the middle and end of the list,
and checks the scroll extent. Companion tests cover keyboard navigation, swipe,
wheel scrolling, resizing, folder changes, animated heights and loading the next
200-row page from a fixture store. Timing is reported, not asserted: these debug
measurements do not establish the release frame-time budget above.

## Conversation queries

Measured with `cargo test -p lagom-core conversation_page_does_not_scan -- --nocapture`
in a debug build on macOS, using 100,000 fixture messages in about 50,000 threads
and a 200-conversation page. No app or native view is started.

| Query                        |                                Before |           Indexed page |
| ---------------------------- | ------------------------------------: | ---------------------: |
| Inbox                        | 176.63 ms / 4,169,009 SQLite VM steps | 2.95 ms / 34,912 steps |
| Later (all messages snoozed) |                                     — | 2.97 ms / 32,520 steps |

The regression test asserts fewer than 100,000 VM steps, independent of machine
speed. The query walks the date index to select each thread's latest visible
message, then reads only the selected threads through the thread index. Schema
v9 adds a partial date index for Later. These query measurements exclude account
counts and rendering; they do not establish the full reload or frame budget.

## Address autocomplete

`cargo test -p lagom-core --release lookup_50k_messages_under_5ms -- --nocapture`
builds a disposable store with 50,000 messages and 50,000 distinct correspondents.
A release measurement on macOS: 240 ms to read, rank and index the snapshot;
0.2–1.6 µs for six common, selective, accent-folded and missing prefix lookups.
The test asserts each lookup takes less than 5 ms and verifies result counts.
The budget excludes building the index, rendering and executor scheduling.

Compose builds this account-local snapshot on the background executor when opened,
then reads only the in-memory prefix index while typing. Reopening refreshes it;
an already open sheet keeps its snapshot until reopened. Dormant restored drafts
do not build an index. No network call, migration or real account is involved.

## Window reload

Measured 2026-11-14 on macOS 27.2.1, Apple M4 Max. Best of 5 runs, `--test-threads=1`.
Both tests are ignored by default:

- `cargo test -p lagom [--release] reload_reads_100k -- --ignored --nocapture` times the
  store reads of one reload. Fixture: two accounts with Inbox, Archive and Sent. Personal
  holds 100,000 messages (50k Inbox, 40k Archive, 10k Sent, two of three unread). Work holds three.
- `cargo test -p lagom [--release] reload_ui_thread_100k -- --ignored --nocapture` times
  `reload()` on the UI thread in GPUI's headless `TestAppContext`. Fixture: one account,
  100,000 Inbox messages. The inline row runs the reads on the UI thread, as before this change.

| What                             |    Debug before | Debug after | Release before | Release after |
| -------------------------------- | --------------: | ----------: | -------------: | ------------: |
| `load_accounts` (background)     |          234 ms |       80 ms |       70–97 ms |         23 ms |
| `folders()` for Work, 3 messages |     not measured |     0.03 ms |          15 ms |       0.01 ms |
| `reload()` on the UI thread      | 129 ms (inline) |     0.31 ms | 39 ms (inline) |       0.09 ms |

`folders()` scanned every message once per folder: the `f.id` column's affinity stopped
SQLite from seeking the `coalesce(move_target,folder)` index. `+f.id` removes the
affinity; `folder_unread_counts_seek_each_folder` asserts the query plan in CI.
`load_accounts` also reuses the current account's folders instead of reading them twice.

The read and the Home unread count now run on the background executor. The old list
stays on screen until the read lands. One list read runs at a time, and requests made
meanwhile merge into one follow-up read. A generation counter drops results older than the
list on screen. A read for another list, or one that finishes while a move is in flight,
is dropped. `list_reads_run_off_the_ui_thread_and_never_go_back_in_time` covers these rules.

Still counted per reload: unread per folder and the Inbox and Later totals each visit
every row of their folder (about 10–13 ms each in release at 100k). A covering partial
index needs a schema bump. Not measured: native frame times, a real account.
