# Todo

## Brand and site

- [x] Domain: `lagommail.app`, chosen 2026-11-09 (`lagom.app`, `.se`, `.dev` and `.email` are taken). Bundle id `app.lagommail.desktop`. Registration not confirmed in the registry on 2026-11-09.
- [x] Licence: MIT.
- [ ] Trademark and competitor search for "Lagom" in classes 9, 38 and 42. Web pass done 2026-11-08: `docs/trademark-check.md` (medium risk, official databases still to search by hand).
- [x] App icon: Blender source `brand/app-icon/skymning/3d/skymning.py`; platform assets rebuilt with `brand/app-icon/export.sh`.
- [x] Windows: embed `brand/app-icon/export/lagom.ico` as executable resource 1, also used by GPUI for its window icon; headless Windows test compares every embedded image with the source.
- [x] Brand concept: Concept D "Butik" from Claude design.
- [x] Exact colour tokens from Claude design, in `apps/desktop/src/theme.rs` (from ConceptD and MainWindowD).
- [ ] Website: switch `site/` to the exact tokens and drop the rejected parts (0 kr cards, info strip, pill buttons).
- [x] Website in `site/`, Concept D, deployed with GitHub Pages.
- [ ] Website: point `lagommail.app` at GitHub Pages (DNS at the registrar, then the Pages custom domain and HTTPS).

## Build

- [x] Rust workspace skeleton.
- [ ] Formatting baseline: `cargo fmt --check` reports existing style differences; no formatter configuration or CI routine is established.
- [x] Desktop UI framework: GPUI.
- [x] Build and start on macOS (2026-11-06).
- [ ] Build on Linux and Windows (CI).
- [ ] Native iPhone and iPad app (SwiftUI) in `apps/mobile/apple`. Planned for the night run 2026-11-06.
- [ ] Native Android phone and tablet app (Jetpack Compose) in `apps/mobile/android`. Planned for the night run 2026-11-06.
- [ ] UniFFI bindings from `lagom-core` for both mobile apps.

## Mail (v1)
- [x] All/Unread filters loaded message rows; multi-select supports Shift+arrows, Shift-click, Cmd-click, Cmd+A and batch actions. Integrated with Google Calendar/Today, Dusk (accent Done last, 140 ms glide) and Settings switches, retaining both branches' regression tests.

- [x] Send Markdown mail (lists, bold/italic, links, headings, quotes, code), compose shortcuts and file insertion, CLI `--body-file` (`feat/markdown-compose`).
- [ ] Show Markdown styling live in the composer (bold text bold, markers dimmed).
- [x] Everyday mail: Forward (F, toolbar, palette, pop-out) with quoted headers and cached/on-demand attachments; Reply all (Shift+R, reply menu, palette) removes account identities and duplicate recipients; read/unread (Shift+U, palette) shares the multi-select batch path and Undo; per-account Markdown signatures in Settings, inserted once above quotes. Merged main with multi-select, Unread, Google calendar, Dusk and Settings switches. Schema v11 follows the calendar schema on main and preserves existing data. Review fixes use account/store identities for read callbacks, discard account-specific Undo on switching, and retain confirmed database flags across queued failures. Forward/Reply all preserve saved drafts; attachment completion uses background autosave. Fixture/headless regressions cover account switches, consecutive rejected flags, explicit compositions, locked draft storage, multi-select shortcuts and filtered popouts, alongside drafts, MIME, attachments and migration.

- [x] Reading and Settings review: Reply/Reply all, Forward and Later lead both reading toolbars; Done stays last in the accent. Single-message Move/read buttons removed; M and the palette open Move, Shift+U toggles read status, and multi-select retains its actions. Account signatures appear in live and demo Settings with the new-mail/reply/forward placeholder, save on blur through the existing store, and stay in memory in demo mode. Hide the default-task-list block when no lists exist. Headless regressions cover toolbar order/actions, shortcuts, multiline demo signatures, per-account blur persistence and empty/nonempty task lists.

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
- [x] Trash: system sidebar entry mapped through IMAP special-use flags, confirmed Empty Trash on the worker (UIDPLUS required), and Move to Inbox. Display and empty use the same folder ID; emptying waits for pending operations across window reopening and a successful list read. App-scoped Empty Trash metadata blocks Restore after reopening and preserves completion cleanup of newly opened popouts. Popouts retain Move to Inbox for messages outside the loaded page. Restore clears `\Deleted` under the UIDVALIDITY guard after interrupted emptying; custom children of system folders stay visible. SEARCH has one command deadline. Fixture regressions cover the review fixes and coexistence with inline folder creation, including pending-operation and stale-snapshot guards. Delete moves to server Trash; demo mode keeps it in memory.
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
- [x] Message list: Shift-arrow/Shift-click ranges, Cmd-click toggles, Cmd+A and Escape, stable selection across sync, conversation-aware batch Done/Later/Delete/Move/read status with one Undo, and a selection summary. All/Unread filters the loaded rows. Fixture and headless tests cover selection, filtering, batch undo and worker read flags. Review fixes bind rollback to worker operations, retain selected mail past page boundaries, reveal search targets, clear the hidden Trash filter, keep the sole selection readable after Cmd-click, share thread read state between labels and actions, reject failed IMAP read-status commands, retain Undo selection across replaced store rows, link HTML tasks to the sole selection, and reveal filtered popout targets before actions.

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

- [x] Calendar slice b1: read-only Google agenda above local tasks in Today, with a quiet consent row. Explicit full desktop consent for mail/identity/calendar/tasks, actual granted scopes, schema v10, paged calendar/event fetch, two-minute background window refresh without blocking mail commands, cached events retained outside each replacement window, DST-safe day boundaries, all-day dates and meeting links. Fixture and headless regression coverage; live provider consent and visual layout remain unverified.
- [x] Calendar review: a complete calendar list hides missing calendars from Today while retaining their cached events; fixture regression covers the fetch-to-store path and account isolation.
- [ ] Microsoft calendar agenda in Today (slice c).
- [ ] Create a meeting from the app with Google Meet or Teams attached, without opening Gmail or Outlook. Zoom is outside v1 (2026-11-09).

## Tasks

- [x] Core: local task lists and tasks, mail link, CLI (feat/tasks-core)
- [x] Local Today view at the top of the sidebar, including without an account: open undated, overdue and today tasks grouped by list, completion checkbox, inline add/rename, keyboard selection and reorder. Add/rename/delete lists; deletion moves every task to the persisted default list, which Settings selects and deletion protects. Schema v9 preserves existing task data and lists.
- [x] `t` in the message list and reading pane and ⌘K "New task" create a task in the default list with an Undo toast. Links resolve by account + Message-ID across folder moves; missing mail gives a toast. Store fixtures and headless dispatch tests cover creation/undo, linked mail, list deletion, settings, migration and Today editing.
- [x] Tasks review: defer linked-mail navigation until pending moves finish; commit task and list edits directly to SQLite on the UI side, independent of the mail worker, and retain editor text with an error toast on failure; preserve Undo Send on Today and the HTML body during task toasts. Limit task-write lock waits to 250 ms, restore the UI connection's timeout afterwards and leave the worker's 5-second timeout unchanged. Fixture/headless regressions cover competing SQLite writers during creation/completion/Undo, retained editor text and retry, timeout restoration, stalled mail operations, window reopening, immediate quit, storage failure recovery and the earlier navigation/Undo/HTML fixes.
- [ ] Tasks: native keyboard/focus, HTML bridge and layout acceptance on macOS/Windows/Linux; headless tests only. Today currently lays out every open task; virtualize if large lists require it.
- [x] Tasks slice b2: two-way Google Tasks sync, schema v13 metadata/tombstones and detached-list ownership, name mapping, push before incremental pull, local edits win, notes mail links, debounced desktop edits and two-minute sync, per-account status. Review fixes protect Google’s default list and recover rejected deletions, accept bounded full-size task pages, provide Tasks consent independently of Calendar access, and run task timers/completions during stalled mail commands. Loopback fixtures, migration and headless regressions cover all five findings; live Google acceptance remains unverified.
- [ ] Microsoft To Do sync (slice c).

## Mail core

- [x] Accounts, OS keychain app passwords and configurable provider presets.
- [x] Migrated SQLite/WAL store, FTS5 search, paged metadata and local Later/snooze queries.
- [x] IMAP folder discovery, bounded initial and incremental sync, flags, expunges, IDLE and mail operations.
- [x] IMAP BODYSTRUCTURE nesting: skip text branches beyond depth 32 as attachments; reject structured responses above 40 nested lists and independently cap parser recursion at 40 (including unsolicited and fragmented FETCH, message bodies and extensions). Local imap-proto patch prevents quoted unknown attributes from bypassing the wire scanner; remove the patch when upstream bounds recursion.
- [x] SMTP submission and Sent APPEND; Gmail saves its own Sent copy.
- [x] `lagom-cli` and disposable GreenMail integration coverage.
- [x] Outbox in the store: persist drafts and pending sends; retry temporary failures on sync with exponential backoff (1 min–1 h), stop permanent failures, and require deliberate retry for uncertain delivery. Failed sends offer Send again and atomic return to draft. Checkpoint SMTP and Sent APPEND separately; CLI sends leave no queue entries. Quit needs no network wait or unsent-mail warning.
- [x] Google OAuth2 (XOAUTH2), including same-account re-login from the sidebar/account rows, refresh-token replacement and sync/outbox resumption. OAuth continues during other accounts' syncs and remains cancellable, covered by fixture tests. Release client IDs can be embedded with `scripts/build-release.sh` and git-ignored `.env.oauth`; malformed assignments stop the build even before later valid lines, tested on Bash 3.2 and 5.
- [ ] Microsoft OAuth2: flow and XOAUTH2 implemented; configure the Entra app registration and verify provider acceptance.
- [ ] JMAP.
- [ ] Push beyond IMAP IDLE.
- [x] HTML rendering on macOS and Windows (`feat/html-mail`): one wry web view per window, page JavaScript off, a strict CSP, no navigation, links to the system browser, "Load remote images" per message. Linux shows the text part.
- [ ] HTML on Linux: wry's child web view needs GTK's main loop and X11; GPUI runs neither.
- [ ] HTML: inline `cid:` images. Sync fetches only the text and HTML parts; fetch the inline parts and serve them through a custom protocol.
- [ ] HTML: mail synced before `feat/html-mail` has no stored HTML part (multipart mail kept only its text part). Refetch bodies once, or on open.
- [ ] HTML: check on Windows (WebView2: do app scripts run with page scripts off, so links and Esc work?).
- [ ] HTML: wide newsletters scroll sideways in a narrow pane; Apple Mail scales them to fit.
- [x] Attachment download: BODYSTRUCTURE metadata, on-demand decoding on the worker, bounded incoming bytes, Windows-safe length-limited unique Downloads files and reveal; backfill all unclassified cached messages. Attachment schema v7 follows drafts/outbox v5–v6, with data-preserving upgrade fixtures from v4, v5 and v6. FETCH selection requires both the UID and requested BODY section, including preceding FLAGS-only responses. Fixture regressions and GreenMail coverage. Encoded parts over 64 MiB need streaming.
- [x] Threading UI: schema v8 stores account-scoped conversations from Message-ID, References and In-Reply-To, with subject/participants fallback only without those headers. Folder lists page and virtualise conversation rows, show latest sender/snippet, aggregate unread and count, and move/snooze/delete/undo every folder member. Reading shows the whole cached thread oldest first, with expandable bodies and existing HTML/attachment rendering. Settings can disable grouping (on by default). Fixture and headless tests cover chains, broken references, migration, paging, actions/undo and virtualisation. Review fixes deduplicate folder copies only for reading, expand the first reply without advancing on receipts, preserve HTML document bounds under native clipping, and page by the actual latest (date, id). One schema v8 migration preserves v7 data and adds the thread, Later date and folder-aware indexes. Indexed page reads deduplicate visible threads without repeated newest-member scans; regressions cover 100,000-message Inbox/Later fixtures and 2,000 visible messages with 2,000 newer cross-folder or snooze-hidden replies. Paging comparisons cover accounts, folders, deadlines and date/id ties. Popout actions resolve conversation members after new replies, covered for Done/Later/Delete in the current list and after switching lists. Merged current main while retaining the app icon, bundle identity, notifications, Trash, folder creation, outbox and attachments; schema remains one v8 migration above v7. Headless regressions cover inactive conversation read state, notification clicks into older thread members and preference compatibility across both branches.
- [ ] Threading: cached v7 rows lack complete References/In-Reply-To. Migration uses stored Message-ID/thread_key; bounded sync refetches missing metadata on subsequent passes. Threads are complete only for locally synced mail.
- [ ] Threading: prefer Gmail X-GM-THRID when async-imap exposes an accessor (the current parser recognises it but Fetch does not expose it).
- [ ] Threading: HTML clipping geometry has regression coverage; native scrolling/clipping and expanded-body layout still need manual macOS/Windows acceptance; expanded bodies currently have their own scroll areas.
- [x] Desktop on the core (`feat/real-mail`): a worker thread owns network and store writes; the UI reads the store. Add account (Gmail, iCloud, Fastmail, Outlook, Other IMAP) with IMAP and SMTP login check, launch, 2-minute and IDLE sync, real folders and counts, optimistic Done/Delete/Move/Later with revert and undo on the server, Send after the 5 s Undo window, Reply headers, C and ⌘N, ⌘K store search, Settings with in-app remove. `LAGOM_DEMO=1` keeps the sample mail. GreenMail covers the worker end to end (`LAGOM_GREENMAIL=1 cargo test -p lagom greenmail`).
- [ ] Desktop: try by hand with a real account. Only GreenMail, tests and screenshots cover it; nobody has clicked or typed in the live window.
- [x] Notifications for new mail: macOS UserNotifications (lazy permission) and Linux D-Bus, sender/subject or batch summary, click to message/Inbox, persisted On/Off setting. First sync, UIDVALIDITY resets, read, Later and Done mail are silent. All worker Inbox sync paths, including moves and undo, detect new mail. UI revalidation reads metadata only; pending permission batches stay per account. Notification handling and the single mail worker live at application scope; new mail and pending permission batches still post with the main window closed, respecting the saved setting and current message metadata; Dock and notification reopening reuse the worker and store without recovering active outbox sends. Operation IDs and pending-command guards survive reopening so old completions cannot match new actions or allow premature Empty Trash; completions release the guard even with no window. Clicks select the target. Fixture and headless tests cover delivery without a window, window release/reopen with the real worker, in-flight outbox protection, selection, activation and caller-level metadata-only revalidation.
- [ ] Notifications: Linux cold-start activation via a desktop entry; current click callbacks need Lagom running.
- [ ] Notifications: Windows toast app identity and activation registration with the installer; no Windows notifications yet.
- [ ] Notifications: verify permission, delivery and clicks in a macOS app bundle and on a Linux notification server; no GUI acceptance run yet.
- [ ] Outlook preset: password login is unsupported; use Microsoft browser sign-in after configuring the Entra registration. Replace the password form with the provider sign-in action during onboarding.
- [ ] Bodies over 4 MiB are stored as headers with a "not downloaded" body; fetch on demand.
- [ ] Undo of a move for a message without Message-ID fails with a toast (the worker finds moved mail by Message-ID).
- [x] Message list virtualisation: visible rows plus overscan; height animation, selection glide, swipe, keyboard scrolling and paging covered by headless tests. Undo plus swipe keeps row heights consistent; spring-back keeps its progress across off-screen rows. 10,000-row measurement in `docs/performance.md`.
- [ ] Store reads on the UI thread: each reload runs account-wide count queries. Codex measured about 37 ms at 100k rows (Python SQLite, not the app). Build the snapshot on the worker before large mailboxes.
- [x] Undo restores the previous snooze deadline after Done/Delete/Move, including every member of a conversation. Headless tests verify the queued restore commands and completion reloads with replaced store IDs, retained single/batch selection, page boundaries and subsequent actions.
- [ ] A server MOVE that succeeds but whose follow-up folder sync fails shows as "Not moved"; the next sync shows the truth.
- [ ] Real-provider TLS/STARTTLS and native keychain acceptance testing, Linux/Windows builds, remaining performance budgets.
