# Performance

Goal: the most optimised email client there is. Every number below is a budget that CI measures. A change that breaks a budget does not merge.

## Budgets

The targets are ours, set 2026-11-06. CI measures the parts that run without a window; see [CI gate](#ci-gate).

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
| `folders()` for Work, 3 messages |    not measured |     0.03 ms |          15 ms |       0.01 ms |
| `reload()` on the UI thread      | 129 ms (inline) |     0.31 ms | 39 ms (inline) |       0.09 ms |

`folders()` scanned every message once per folder: the `f.id` column's affinity stopped
SQLite from seeking the `coalesce(move_target,folder)` index. `+f.id` removes the
affinity; `folder_unread_counts_seek_each_folder` asserts the query plan in CI.
`load_accounts` also reuses the current account's folders instead of reading them twice.

The read and the Home unread count now run on the background executor. The old list
stays on screen until the read lands. One list read runs at a time, and requests made
meanwhile merge into one follow-up read. A generation counter drops results older than the
list on screen. Starting a move or another operation also invalidates reads already
running. A read for another list is dropped. So is one that finishes while a move is in
flight, or one that would drop a row selected while it ran (that row is read again, pinned).
A jump to one message (search hit, notification, task link, Move from a popout, Move for a
selection in another account) reads on the background executor too. Its next steps need
that account's folders and rows, so they run in the read's completion: when the account's
folders are not on screen yet, the jump waits for that read; then it opens the message's
list, and the next read selects the message and runs the follow-up (the Move palette, or
the popout's Done or Later). The old list stays on screen until then. Rules: the account's
folders come from a read requested after the message was found; nothing is selected between
opening the list and selecting the message, so no key or render acts on row 0; another
account, list or section cancels the pending jump; a read error ends it; a new jump is refused with
"Wait for the message to open" while one with a follow-up is pending.

`reload_ui_thread_100k` also times the jump (`reload.ui_thread_jump`): `open_found` for an
old Inbox message at 100k, release, M4 Max, best of 5, three runs each. Before
(`reload_now` inline): 25.7–26.6 ms. After: 0.88–1.08 ms. Tests:
`list_reads_run_off_the_ui_thread_and_never_go_back_in_time`,
`a_row_selected_during_a_read_stays_listed`,
`search_hit_in_a_folder_opens_while_a_read_is_in_flight` (the jump keeps the old list,
then its completion selects the hit and opens the Move palette) and
`a_jump_selects_nothing_until_its_read_lands_and_navigation_cancels_it`.

Still counted per reload: unread per folder and the Inbox and Later totals each visit
every row of their folder (about 10–13 ms each in release at 100k). A covering partial
index needs a schema bump. Not measured: native frame times, a real account.

## CI gate

`.github/workflows/perf.yml` runs `python3 scripts/perf.py` on ubuntu-latest and macos-latest
for pull requests that touch Rust code, and for every push to main. The script runs the ignored
benchmarks in release (`lagom-core` `perf_tests`, and `reload_reads_100k` and
`reload_ui_thread_100k` in `lagom`), builds the release binary, and writes a table to the job
summary. Each time is the best of five runs.

The numbers live in `docs/performance-budgets.toml`. A run fails when a time is above
budget × 2, a size is above its budget, or a budgeted key is missing. The factor 2 covers
GitHub's shared runners, which are slower and noisier than the reference Mac. Measurements
without a budget are listed as "no budget".

What the gate cannot measure: first presented frame, frame times, resident memory and idle CPU.
These need a window and a GPU. `reload.ui_thread_background` and `reload.ui_thread_jump`
time the `reload()` and `open_found()` calls only;
applying the finished read to the list also runs on the UI thread and is not timed. "Cold start" here is store open plus the first list read on a
warm OS file cache, in an already running process.

Measured 2026-11-14 on macOS 27.2.1, Apple M4 Max, release, other builds running meanwhile:

| Key                            |     Value | Budget |
| ------------------------------ | --------: | -----: |
| `search.*` (four queries)      |  ≤ 0.1 ms |  50 ms |
| `open.message`                 |   0.02 ms |  16 ms |
| `reload.ui_thread_background`  |   0.17 ms |   8 ms |
| `reload.ui_thread_jump`        |   0.92 ms |   8 ms |
| `reload.cold_start_first_list` |     44 ms | 200 ms |
| `reload.load_accounts`         |     42 ms | 100 ms |
| `reload.folders`               |     18 ms |  30 ms |
| `reload.counts`                |     15 ms |  30 ms |
| `mime.typical` (25 KB)         |   0.53 ms |   2 ms |
| `mime.large` (3 MB)            |   0.56 ms |  50 ms |
| `html.to_text_100kb`           |    2.0 ms |  10 ms |
| `sync.parse_save_1000`         | 0.8–1.5 s |    2 s |
| `binary.size`                  |   27.5 MB |  32 MB |

First CI run (PR #131, 2026-11-14): every time was within 1.3× of the reference Mac, most
faster. ubuntu-latest: cold start 36 ms, `load_accounts` 35 ms, `sync.parse_save_1000` 1.41 s.
macos-latest: cold start 29 ms, `load_accounts` 28 ms, `sync.parse_save_1000` 1.11 s.

Findings:

- The Linux release binary was 53.2 MB on ubuntu-latest (PR #131), over its 30 MB budget.
  `.text` was 27.4 MB: gpui 13 %, zbus 11.6 %, std 9.6 %, naga 5.3 % (`cargo bloat --crates`);
  the symbol table was 13.6 MB. Cut on 2026-11-15, each step measured on ubuntu-latest in CI
  (the run that measured each size in brackets):

  | Step                                                            | Linux binary           |
  | --------------------------------------------------------------- | ---------------------- |
  | Before                                                          | 53.2 MB (43613223446)  |
  | One zbus: notify-rust held at 4.18, the app on zbus 5 like GPUI | 53.0 MB (43705119901)  |
  | `lto = "fat"`, `codegen-units = 1`                              | 35.7 MB (43705119901)  |
  | Desktop binary without its symbol table (`strip = "symbols"`)   | 30.7 MB (43705119901)  |
  | GPUI's `image` with only the formats in GPUI's `ImageFormat`    | 30.0 MB (43710105708)  |
  | GPUI on ashpd 0.12, the version oo7 uses                        | 29.98 MB (43711959948) |
  | zbus, zvariant, oo7 and ashpd at `opt-level = "s"`              | 29.89 MB (43715180813) |

  Thin LTO alone gave nothing (52.9 MB). The release build of the binary takes about a quarter
  longer (428 s to 541 s on ubuntu-latest). Panic backtraces of the desktop app now show
  addresses, not function names; nothing reads them (no crash reporter, no symbol upload).
  `lagom-ffi` keeps its symbols, because uniffi-bindgen reads its metadata from them. macOS went
  from 27.4 to 16.5 MB. Still in the binary: two fontdb, rustybuzz and ttf-parser versions
  (cosmic-text and resvg), and GPUI's HTTP client (reqwest, hyper). The margin under 30 MB is
  about 0.1 MB, so new Linux code can break the gate again.

- A jump to one message read inline and held the UI thread for 69 ms at 100k
  (`reload.ui_thread_inline`, an earlier run). It now reads on the background executor:
  `reload.ui_thread_jump` is 0.9–1.1 ms (was 25.7–26.6 ms for the same jump), budget 8 ms.
  `reload.ui_thread_inline` stays in the table as the "before" reference for `reload()`
  (38–42 ms) and has no budget.
