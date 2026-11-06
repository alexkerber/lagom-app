# Todo

## Brand and site

- [ ] Domain. `lagom.app`, `lagom.com` and `lagom.io` are taken (RDAP, 2026-11-06). Free on that date: `lagom-mail.com/.app/.io`, `lagomhq.com/.app/.io`, `lagommail.app/.io`, and `.se` for lagom-app, lagommail, lagom-mail and lagomhq.
- [x] Licence: MIT.
- [ ] Trademark and competitor search for "Lagom" in classes 9, 38 and 42.
- [x] Brand concept: Concept D "Butik" from Claude design.
- [x] Exact colour tokens from Claude design, in `apps/desktop/src/theme.rs` (from ConceptD and MainWindowD).
- [ ] Website: switch `site/` to the exact tokens and drop the parts Alex rejected (0 kr cards, info strip, pill buttons).
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

- [x] Main window, Concept D, light and dark (static sample data).
- [x] Native macOS menu bar: lagom, File, View, Window, Help.
- [ ] Edit menu (Cut, Copy, Paste, Select All) once compose has real text fields.
- [ ] Command palette, compose and empty inbox: structure from B, dressed in D tokens.
- [ ] Typeface trial: Hauora Sans against Inter (Alex 2026-11-06).
- [ ] Performance: CI that measures the budgets in `docs/performance.md` (Alex 2026-11-06: "the most optimised mail app").
- [ ] Liquid Glass spike: native `NSGlassEffectView` behind the GPUI sidebar on macOS 26+, blur fallback.
- [x] Window drags by its top edge on macOS (Alex verified by hand, 2026-11-06).

- [ ] Sort mail into custom folders (Alex 2026-11-06: one of the two must-have features). Sidebar group and ⌘K "move <folder>" work on sample data (`feat/folders`); real IMAP/Gmail sync is still open.
- [ ] Folders: the "+" in the heading and the "New folder" row do nothing yet. Wire them to an inline name field once the app has text inputs; ⌘K "move <name>" creates a folder today.
- [ ] Snooze, shown as "Later" in the UI (Alex 2026-11-06: the other must-have feature).
- [x] Swipe a row with two fingers (trackpad, Magic Mouse) to Done or Later; Esc springs it back (Alex 2026-11-06).
- [x] Long left swipe deletes to Trash; ⌫ does the same; "Deleted" toast with Undo; swipe background coloured by state (Alex 2026-11-06, `feat/folders`).
- [ ] Swipe: try it by hand on a trackpad, a Magic Mouse and a mouse drag (incl. release outside the window), including the 245 px delete stage and the colour fade; tests cover only the state logic.
- [ ] Swipe: haptic tick at each threshold. GPUI 0.2.2 has no API for `NSHapticFeedbackManager`.
- [ ] Trash: a sidebar entry to open it and empty it. Delete only moves to an in-memory Trash now.
- [ ] Swipe: GPUI maps `NSEventPhaseCancelled` to Moved, so a system-cancelled gesture leaves the row offset until the next swipe or Esc. Patch or wrap GPUI's phase mapping.
- [x] Swipe: drag a row with the mouse, same stages as the trackpad (Alex 2026-11-06: "ska kunna dra med musen också"). Closed hand while dragging; the arrow at rest, as in native lists (2026-11-06, `feat/feel`).
- [ ] Compose: Esc keeps the draft once fields are editable (now it discards static sample text).
- [ ] Motion: the key moments in `docs/motion.md` (Done, archive, Later, send, undo, inbox zero).
- [x] Feel (Alex 2026-11-06: "de känns så kalla"): hover and press on everything clickable, the selection glides, spring easing, the message list scrolls, Done button in the Done colour, each sample message has its own sender and body (`feat/feel`).
- [ ] Feel: try hover, press, glide, scrolling and swipe-versus-scroll by hand. Only tests and still screenshots cover them.
- [ ] Feel: in the light sidebar, hover looks almost like the selected row (`subtle` is close to `sel`). Decide whether the selected row needs more contrast.
- [x] Typeface: the system font (SF Pro on macOS) is the UI default, chosen by Alex 2026-11-06. `LAGOM_UI_FONT=inter` switches back.
- [ ] Message list: virtualise (visible rows only) when real mail arrives; it renders every row now.

## Folders: open questions

Defaults in the code until Alex answers (Figma "Lagom · Folders", frame 7):

- [ ] Gmail labels and IMAP folders: shown the same way for now. A Gmail message can have several labels.
- [ ] Folders per account (default) or merged across accounts.
- [ ] Shortcuts: only ⌘K for now, no G + letter per folder.
- [ ] Drag a message onto a folder, and a swipe to "Move to": not yet.

## Calendar (Alex 2026-11-06)

- [ ] Agenda next to the inbox: Google and Microsoft calendars.
- [ ] Create a meeting from the app with Google Meet or Zoom attached, without opening Gmail or Google Calendar.

## Tasks (Alex 2026-11-06)

- [ ] Task list on the start screen. Lists "Work" and "Private", plus custom lists.
- [ ] Turn a mail into a task in one key press. The task links back to the mail.
- [ ] Sync tasks with the mail account (for example Google Tasks, Microsoft To Do).

## Mail core

- [x] Accounts, OS keychain app passwords and configurable provider presets.
- [x] Migrated SQLite/WAL store, FTS5 search, paged metadata and local Later/snooze queries.
- [x] IMAP folder discovery, bounded initial and incremental sync, flags, expunges, IDLE and mail operations.
- [x] SMTP submission and Sent APPEND; Gmail saves its own Sent copy.
- [x] `lagom-cli` and disposable GreenMail integration coverage.
- [ ] Gmail/Microsoft OAuth2 (XOAUTH2). Outlook.com requires OAuth2; the preset alone does not enable password login.
- [ ] JMAP.
- [ ] Push beyond IMAP IDLE.
- [ ] HTML rendering (HTML is cached as untrusted content only).
- [ ] Attachment download.
- [ ] Threading UI.
- [ ] Connect desktop UI to the core on workers; desktop remains on sample data in this branch.
- [ ] Real-provider TLS/STARTTLS and native keychain acceptance testing, Linux/Windows builds, remaining performance budgets.
