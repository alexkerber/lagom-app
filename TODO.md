# Todo

## Brand and site

- [ ] Domain. `lagom.app`, `lagom.com` and `lagom.io` are taken (RDAP, 2026-11-06). Free on that date: `lagom-mail.com/.app/.io`, `lagomhq.com/.app/.io`, `lagommail.app/.io`, and `.se` for lagom-app, lagommail, lagom-mail and lagomhq.
- [x] Licence: MIT.
- [ ] Trademark and competitor search for "Lagom" in classes 9, 38 and 42. Web pass done 2026-11-08: `docs/trademark-check.md` (medium risk, official databases still to search by hand).
- [x] App icon: Blender source `brand/app-icon/skymning/3d/skymning.py`; platform assets rebuilt with `brand/app-icon/export.sh`.
- [x] Windows: embed `brand/app-icon/export/lagom.ico` as executable resource 1, also used by GPUI for its window icon; headless Windows test compares every embedded image with the source.
- [x] Brand concept: Concept D "Butik" from Claude design.
- [x] Exact colour tokens from Claude design, in `apps/desktop/src/theme.rs` (from ConceptD and MainWindowD).
- [ ] Website: switch `site/` to the exact tokens and drop the rejected parts (0 kr cards, info strip, pill buttons).
- [x] Website in `site/`, Concept D, deployed with GitHub Pages.
- [ ] Website: point the domain at GitHub Pages.

## Build

- [x] Rust workspace skeleton.
- [x] Desktop UI framework: GPUI.
- [x] Build and start on macOS (2026-11-06).
- [ ] Build on Linux and Windows (CI).
- [ ] Native iPhone and iPad app (SwiftUI) in `apps/mobile/apple`. Planned for the night run 2026-11-06.
- [ ] Native Android phone and tablet app (Jetpack Compose) in `apps/mobile/android`. Planned for the night run 2026-11-06.
- [ ] UniFFI bindings from `lagom-core` for both mobile apps.

## Mail (v1)

- [x] Send Markdown mail (lists, bold/italic, links, headings, quotes, code), compose shortcuts and file insertion, CLI `--body-file` (`feat/markdown-compose`).
- [ ] Show Markdown styling live in the composer (bold text bold, markers dimmed).

- [x] Main window, Concept D, light and dark (static sample data).
- [x] Native macOS menu bar: lagom, File, View, Window, Help.
- [x] Edit menu: Undo, Redo, Cut, Copy, Paste, Select All. A focused text field handles them; Undo falls back to the inbox (`feat/text-input`).
- [ ] Command palette, compose and empty inbox: structure from B, dressed in D tokens.
- [ ] Typeface trial: Hauora Sans against Inter.
- [ ] Performance: CI that measures the budgets in `docs/performance.md` (goal: the most optimised mail app).
- [ ] Liquid Glass spike: native `NSGlassEffectView` behind the GPUI sidebar on macOS 26+, blur fallback.
- [x] Window drags by its top edge on macOS (verified by hand).

- [ ] Sort mail into custom folders (must-have). Sidebar group and ⌘K "move <folder>" work on sample data (`feat/folders`); real IMAP/Gmail sync is still open.
- [x] Folders: the heading "+" and "New folder" open an inline name field (Enter creates, Esc cancels). Shares server creation with ⌘K "move <name>"; optimistic row, rollback/toast on failure, empty/delimiter/duplicate validation. Creation waits for the account's folder list; unknown and NIL delimiters stay distinct. Scrollable folder rows reveal newly created folders; headless coverage with 100 existing folders, failed initial LIST and exact worker validation errors.
- [x] Snooze, shown as "Later" in the UI (must-have). Local only: L/← until tomorrow 08:00; ⌘K offers 18:00, tomorrow 08:00 and Monday 08:00; due mail returns to Inbox (`feat/real-mail`).
- [x] Swipe a row with two fingers (trackpad, Magic Mouse) to Done or Later; Esc springs it back.
- [x] Long left swipe deletes to Trash; ⌫ does the same; "Deleted" toast with Undo; swipe background coloured by state (`feat/folders`).
- [ ] Swipe: try it by hand on a trackpad, a Magic Mouse and a mouse drag (incl. release outside the window), including the 245 px delete stage and the colour fade; tests cover only the state logic.
- [ ] Swipe: haptic tick at each threshold. GPUI 0.2.2 has no API for `NSHapticFeedbackManager`.
- [x] Trash: system sidebar entry mapped through IMAP special-use flags, confirmed Empty Trash on the worker (UIDPLUS required), and Move to Inbox. Display and empty use the same folder ID; emptying waits for pending operations and a successful list read. Popouts retain Move to Inbox for messages outside the loaded page. Restore clears `\Deleted` under the UIDVALIDITY guard after interrupted emptying; custom children of system folders stay visible. SEARCH has one command deadline. Fixture regressions cover the review fixes and coexistence with inline folder creation, including pending-operation and stale-snapshot guards. Delete moves to server Trash; demo mode keeps it in memory.
- [ ] Swipe: GPUI maps `NSEventPhaseCancelled` to Moved, so a system-cancelled gesture leaves the row offset until the next swipe or Esc. Patch or wrap GPUI's phase mapping.
- [x] Swipe: drag a row with the mouse, same stages as the trackpad. Closed hand while dragging; the arrow at rest, as in native lists (2026-11-06, `feat/feel`).
- [x] Compose: Esc keeps the draft locally and R brings it back; Send moves it to the outbox and ⌘⌫ discards it.
- [x] Text input: `TextInput` (single and multi-line, selection, word and line keys, clipboard, undo, IME, password mode) in compose and the palette (`feat/text-input`).
- [ ] Text input: try by hand. Typing, an IME (Japanese, Pinyin), dead keys, mouse and double-click selection, drag past the edge, scrolling a long body, the Edit menu. Tests cover the editing model and GPUI key dispatch only.
- [ ] Text input: caret, accent selection and focus ring are not in a screenshot yet. GPUI blurs fields in an inactive window, and the capture window was inactive.
- [x] Compose: debounce autosave for 300 ms and write off the UI thread; revision checks preserve Send/Undo/Discard ordering; restore drafts on start and release retained drafts after account removal.
- [ ] Compose: sync local drafts with the server-side Drafts folder.
- [ ] Motion: the key moments in `docs/motion.md` (Done, archive, Later, send, undo, inbox zero).
- [x] Feel: hover and press on everything clickable, the selection glides, spring easing, the message list scrolls, Done button in the Done colour, each sample message has its own sender and body (`feat/feel`).
- [ ] Feel: try hover, press, glide, scrolling and swipe-versus-scroll by hand. Only tests and still screenshots cover them.
- [x] Selection contrast raised and the sidebar frosted denser. Real blur radius needs native code.
- [ ] Feel: in the light sidebar, hover looks almost like the selected row (`subtle` is close to `sel`). Decide whether the selected row needs more contrast.
- [x] Typeface: the system font (SF Pro on macOS) is the UI default. `LAGOM_UI_FONT=inter` switches back.
- [x] Message list: visible rows plus two-row overscan, with animated-height spacers; integrated with persisted drafts, outbox and non-blocking quit.

## Folders: open questions

Defaults in the code until the maintainer decides (Figma "Lagom · Folders", frame 7):

- [ ] Gmail labels and IMAP folders: shown the same way for now. A Gmail message can have several labels.
- [ ] Folders per account (default) or merged across accounts.
- [ ] Shortcuts: only ⌘K for now, no G + letter per folder.
- [ ] Drag a message onto a folder, and a swipe to "Move to": not yet.

## Product line

- Very fast: mail, meetings and todos in one app. Open source, so people can add to it. No AI inside the app; AI connects from outside through MCP.
- [ ] MCP server on lagom-core, so AI models can read and act on mail, calendar and todos from outside the app.

## Calendar

Spec: `docs/calendar-tasks.md` (2026-11-08). Open decisions are in its last section.

- [ ] Agenda next to the inbox: Google and Microsoft calendars.
- [ ] Create a meeting from the app with Google Meet or Zoom attached, without opening Gmail or Google Calendar.

## Tasks

- [x] Core: local task lists and tasks, mail link, CLI (feat/tasks-core)
- [ ] Task list on the start screen. Lists "Work" and "Private", plus custom lists.
- [ ] Turn a mail into a task in one key press. The task links back to the mail.
- [ ] Sync tasks with the mail account (for example Google Tasks, Microsoft To Do).

## Mail core

- [x] Accounts, OS keychain app passwords and configurable provider presets.
- [x] Migrated SQLite/WAL store, FTS5 search, paged metadata and local Later/snooze queries.
- [x] IMAP folder discovery, bounded initial and incremental sync, flags, expunges, IDLE and mail operations.
- [x] IMAP BODYSTRUCTURE nesting: skip text branches beyond depth 32 as attachments; reject structured responses above 40 nested lists and independently cap parser recursion at 40 (including unsolicited and fragmented FETCH, message bodies and extensions). Local imap-proto patch prevents quoted unknown attributes from bypassing the wire scanner; remove the patch when upstream bounds recursion.
- [x] SMTP submission and Sent APPEND; Gmail saves its own Sent copy.
- [x] `lagom-cli` and disposable GreenMail integration coverage.
- [x] Outbox in the store: persist drafts and pending sends; retry temporary failures on sync with exponential backoff (1 min–1 h), stop permanent failures, and require deliberate retry for uncertain delivery. Failed sends offer Send again and atomic return to draft. Checkpoint SMTP and Sent APPEND separately; CLI sends leave no queue entries. Quit needs no network wait or unsent-mail warning.
- [ ] Gmail/Microsoft OAuth2 (XOAUTH2). Outlook.com requires OAuth2; the preset alone does not enable password login.
- [ ] JMAP.
- [ ] Push beyond IMAP IDLE.
- [x] HTML rendering on macOS and Windows (`feat/html-mail`): one wry web view per window, page JavaScript off, a strict CSP, no navigation, links to the system browser, "Load remote images" per message. Linux shows the text part.
- [ ] HTML on Linux: wry's child web view needs GTK's main loop and X11; GPUI runs neither.
- [ ] HTML: inline `cid:` images. Sync fetches only the text and HTML parts; fetch the inline parts and serve them through a custom protocol.
- [ ] HTML: mail synced before `feat/html-mail` has no stored HTML part (multipart mail kept only its text part). Refetch bodies once, or on open.
- [ ] HTML: check on Windows (WebView2: do app scripts run with page scripts off, so links and Esc work?).
- [ ] HTML: wide newsletters scroll sideways in a narrow pane; Apple Mail scales them to fit.
- [x] Attachment download: BODYSTRUCTURE metadata, on-demand decoding on the worker, bounded incoming bytes, Windows-safe length-limited unique Downloads files and reveal; backfill all unclassified cached messages. Attachment schema v7 follows drafts/outbox v5–v6, with data-preserving upgrade fixtures from v4, v5 and v6. FETCH selection requires both the UID and requested BODY section, including preceding FLAGS-only responses. Fixture regressions and GreenMail coverage. Encoded parts over 64 MiB need streaming.
- [ ] Threading UI.
- [x] Desktop on the core (`feat/real-mail`): a worker thread owns network and store writes; the UI reads the store. Add account (Gmail, iCloud, Fastmail, Outlook, Other IMAP) with IMAP and SMTP login check, launch, 2-minute and IDLE sync, real folders and counts, optimistic Done/Delete/Move/Later with revert and undo on the server, Send after the 5 s Undo window, Reply headers, C and ⌘N, ⌘K store search, Settings with in-app remove. `LAGOM_DEMO=1` keeps the sample mail. GreenMail covers the worker end to end (`LAGOM_GREENMAIL=1 cargo test -p lagom greenmail`).
- [ ] Desktop: try by hand with a real account. Only GreenMail, tests and screenshots cover it; nobody has clicked or typed in the live window.
- [ ] Notifications for new mail.
- [ ] Outlook preset: password login will fail until OAuth2 lands; the form says so in its note only.
- [ ] Bodies over 4 MiB are stored as headers with a "not downloaded" body; fetch on demand.
- [ ] Undo of a move for a message without Message-ID fails with a toast (the worker finds moved mail by Message-ID).
- [x] Message list virtualisation: visible rows plus overscan; height animation, selection glide, swipe, keyboard scrolling and paging covered by headless tests. Undo plus swipe keeps row heights consistent; spring-back keeps its progress across off-screen rows. 10,000-row measurement in `docs/performance.md`.
- [ ] Store reads on the UI thread: each reload runs account-wide count queries. Codex measured about 37 ms at 100k rows (Python SQLite, not the app). Build the snapshot on the worker before large mailboxes.
- [ ] Undo of Done or Delete on a message in Later puts it back in its folder but not on Later: the server move drops the local snooze.
- [ ] A server MOVE that succeeds but whose follow-up folder sync fails shows as "Not moved"; the next sync shows the truth.
- [ ] Real-provider TLS/STARTTLS and native keychain acceptance testing, Linux/Windows builds, remaining performance budgets.
