# Spec: Calendar and Tasks for Lagom

Based on `feat/oauth` at 0309143. Anything I could not check against live docs is marked **not verified**.

## Three things in the current code that shape the design
1. **The token code assumes one scope string per provider.** `oauth.rs` caches one token per account id (`cache(&account.id)`), and on refresh it sends the fixed `config.scope` again.
   - Google: once accounts hold different scope sets, sending scopes the user never granted may break the refresh (**not verified**). Fix: drop `scope` from Google refreshes, which keeps what was granted.
   - Microsoft: a token covers one resource. The IMAP token (`outlook.office.com`) cannot call Graph. Fix: `access_token(account, Resource::{Mail, Graph}, rejected)`, with the cache keyed by `(id, resource)`. Microsoft's refresh token can usually be exchanged for a Graph token after consent (**not verified** for personal accounts).
2. **A mail→task link cannot use `messages.id`.** A move deletes the row and inserts a new one, and Done is a move. Link by `(account, Message-ID)`, which is what the worker already uses to follow moved mail (`sync.rs:422-440`). Also keep a copy of the subject, so the link still makes sense once the mail is gone. Mail with an empty Message-ID gets a task with no link.
3. **Key `t` creates a local task** in slice a, in both the message list and reading pane.

## 1. Provider APIs and scopes
| | Calendar | Tasks |
|---|---|---|
| Google | Calendar v3: `calendarList.list`, `events.list?singleEvents=true&timeMin&timeMax`, `events.insert?conferenceDataVersion=1&sendUpdates=all` | Tasks v1: `tasklists.list`, `tasks.list?showDeleted&showHidden&updatedMin` |
| Scopes | `calendar.events` + `calendar.calendarlist.readonly` (confirmed in the [CalendarList reference](https://developers.google.com/workspace/calendar/api/v3/reference/calendarList/list)), or else `calendar` | `tasks` |
| Microsoft Graph | `/me/calendars`, `/me/calendarView?startDateTime&endDateTime`, `POST /me/events` | `/me/todo/lists`, `/me/todo/lists/{id}/tasks` (a delta endpoint exists) |
| Scopes | `Calendars.ReadWrite offline_access` | `Tasks.ReadWrite` |

**Google consent**
- `https://mail.google.com/` is a restricted scope, so production already needs verification plus a CASA assessment.
- The Calendar and Tasks scopes are only sensitive. They add a justification and a demo video, not another CASA.
- While the app is in Testing mode, refresh tokens expire after 7 days and there is a cap of 100 test users (**not verified** for 2026).
- Users who are already signed in have none of these scopes. Offer a quiet "Show your Google Calendar" row in Today, with events above tasks after consent. Google [does not support incremental authorization for installed apps](https://developers.google.com/identity/protocols/oauth2/native-app) (checked 2026-11-09). Run a new loopback consent with the full mail/identity/calendar/tasks scope set and `login_hint=<email>`; preserve already granted scopes on later sign-ins. Reject the result if the ID-token email does not match the account.
- Google shows granular consent checkboxes, so read the `scope` field in the token response and store what was actually granted.

**Microsoft:** these are delegated scopes a user can consent to alone. Some tenants block user consent (**not verified** per tenant).

**CalDAV / iCloud: leave out of v1.** It needs our own iCalendar parsing and RRULE expansion, iCloud Reminders no longer goes through CalDAV, and server-side expansion support on iCloud is **not verified**.

**Quotas: not verified.** Rough figures: Calendar about 1M requests/day per project, Tasks about 50k/day, Graph throttling around 10k requests per 10 min per mailbox.

## 2. Store
Slice a reuses the task tables shipped in `schema_v4.sql`. Main's latest schema was v8 when checked on 2026-11-09; `schema_v9.sql` adds task creation timestamps and the persisted default-list setting. Existing tasks keep their data, with `created` initialized from `updated`. Existing lists are kept as they are; Work and Private are seeded only when no lists exist. The settings row references the default list, and list deletion moves all tasks to it in one transaction.

The original combined calendar/tasks schema below is the plan for the later network slices; calendar tables and account scopes are not part of slice a.
```sql
ALTER TABLE accounts ADD COLUMN scopes TEXT NOT NULL DEFAULT '';
CREATE TABLE calendars(id INTEGER PRIMARY KEY, account TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
 remote_id TEXT NOT NULL, name TEXT NOT NULL, color TEXT, shown INTEGER NOT NULL DEFAULT 1, writable INTEGER NOT NULL,
 UNIQUE(account,remote_id));
CREATE TABLE events(id INTEGER PRIMARY KEY, calendar INTEGER NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
 remote_id TEXT, title TEXT NOT NULL, start TEXT NOT NULL, end TEXT NOT NULL, start_utc INTEGER NOT NULL,
 all_day INTEGER NOT NULL, location TEXT, join_url TEXT, pending INTEGER NOT NULL DEFAULT 0, UNIQUE(calendar,remote_id));
CREATE INDEX events_start ON events(start_utc);
CREATE TABLE task_lists(id INTEGER PRIMARY KEY, account TEXT REFERENCES accounts(id) ON DELETE CASCADE,
 remote_id TEXT, name TEXT NOT NULL, position INTEGER NOT NULL, UNIQUE(account,remote_id));
INSERT INTO task_lists(name,position) VALUES('Work',0),('Private',1);
CREATE TABLE tasks(id INTEGER PRIMARY KEY, list INTEGER NOT NULL REFERENCES task_lists(id) ON DELETE CASCADE,
 remote_id TEXT, title TEXT NOT NULL, notes TEXT NOT NULL DEFAULT '', due TEXT, done_at INTEGER, position INTEGER,
 mail_account TEXT, mail_message_id TEXT, mail_subject TEXT, updated INTEGER NOT NULL,
 dirty INTEGER NOT NULL DEFAULT 0, deleted INTEGER NOT NULL DEFAULT 0, UNIQUE(list,remote_id));
```
- Local tasks are lists with `account IS NULL`, so tasks work with no account at all.
- `mail_account` has no foreign key on purpose: removing an account must not delete local tasks.
- Times are kept in the provider's own form (`start`/`end`), because all-day dates must not go through UTC. `start_utc` is only for sorting.
- Recurrence: let the provider expand it (`singleEvents` / `calendarView`) and store no RRULE. Mark it in code: `shortcut: no RRULE, add when CalDAV lands`.

Core modules: `crates/lagom-core/src/tasks.rs` and `calendar.rs` (types and store queries, plain `String`/`i64`, ready for UniFFI later), plus `google.rs` and `graph.rs` (JSON over the existing `reqwest`/`serde_json`). No new dependencies.

## 3. Sync model
- **Calendar:** on each worker tick (the existing 2-minute one), re-fetch now−7d to now+60d per shown calendar and replace that window in one transaction. This needs no syncToken or deltaLink and has no state to corrupt. Mark it `shortcut: refetch window, move to syncToken/delta when request counts matter`.
- **Tasks, order of work:** push first, then pull.
  - Push: send rows with `dirty=1` or `deleted=1` as PATCH, DELETE or INSERT, then clear the flags.
  - Pull: Google with `updatedMin` (it has no sync token); Graph by refetching the list (delta later).
- **Conflict rule:** last writer wins, and the local edit wins because it is pushed before the pull. The cost is losing a server edit made in the same 2-minute window. the maintainer should accept this.
- **Offline:** the `dirty`/`deleted`/`pending` columns are the queue. There is no separate ops table.
- **Retries without duplicates:**
  - Google `events.insert` accepts a client-made `id` (base32hex).
  - Graph events take a `transactionId`.
  - Google Tasks has no idempotency key, so a retry after a timeout can create duplicates (**not verified** whether there is a workaround).
- **Mail link when synced:** Graph uses `linkedResources` (`externalId` = Message-ID). Google Tasks `links` is read-only, so add a line to `notes` instead.

## 4. Desktop slices
Each slice ships on its own. Local tasks commit directly through the UI store. Network `Cmd`s go in `apps/desktop/src/sync.rs`, the UI keeps reading the store, and `Event::Changed` is reused.

| Slice | Work | Size |
|---|---|---|
| a. Local tasks + `t` | `tasks.rs` view on the start screen. `t` creates a task in the default list with title = subject, plus the Undo toast. Add/rename lists. Opening a task's link finds the mail by Message-ID. No network. | M |
| b1. Google agenda, read-only | Shipped: full desktop consent, persisted scopes, schema v10, paged `google.rs` window fetch and `agenda.rs` in Today. Google mail and calendar share the same grant/cache; Google refreshes omit scope. | M |
| b2. Google Tasks | Provider task lists and push/pull task sync. | M |
| c. Microsoft | Graph token, `graph.rs` calendarView and To Do. | M |
| d. Create/edit events | Form with `conferenceData.createRequest{requestId, conferenceSolutionKey:{type:"hangoutsMeet"}}`. On Graph, `isOnlineMeeting` (Teams only for work accounts, **not verified** for personal). | L |
| d′. Zoom (after v1) | Excluded from v1; would need a separate OAuth integration and review. | L, separate |

## 5. Tests (no real accounts)
- Fake HTTP server on a loopback `TcpListener`, the way `oauth/tests.rs` does it.
- JSON fixtures under `crates/lagom-core/src/{google,graph}/fixtures/`, with the base URL passed in like `sign_in_with(config)`.
- Cases:
  - paging
  - 401 → one refresh
  - 403 insufficient scope → "Allow calendar access"
  - all-day events across time zones
  - cancelled instances of recurring events
  - retry after a timeout does not duplicate
  - dirty push survives a failed pull
  - v3→current migration seeds Work/Private; v8→v9 preserves existing lists, tasks and timestamps
  - removing an account keeps local tasks
- Desktop: a `TestAppContext` test where `t` creates a task (same pattern as `main_window.rs:2372`).

## 6. Maintainer decisions

Decided on **2026-11-09**:
1. **Start screen:** Home (⌘1) opens today's events, due/undated open tasks and the five newest unread Inbox messages across accounts. Mail (⌘2) opens Inbox; Calendar (⌘3) opens the cached week; Todo (⌘4) shows open tasks from every list, including future tasks. Mailboxes, Folders and Accounts remain below the top group. The sample account is Personal.
2. **Lists:** provider lists appear unchanged under each account in slice b. Locally, create Work and Private only when no lists exist.
3. **Mail → task:** `t` on a mail creates a task immediately in the default list, with title = subject and a link by account + Message-ID. Show an Undo toast. Choose the default list in Settings. A mail without Message-ID creates an unlinked task.
4. **Deleting a list:** move its tasks to the default list. Delete an empty list immediately. The default list cannot be deleted.
5. **Scopes:** request all calendar and tasks scopes once in slice b.
6. **Zoom:** not in v1.

Slice a behavior: Today shows open tasks due today or earlier, plus undated tasks, grouped by list. Future-dated, completed and deleted tasks stay out of Today. It works without a mail account. Enter renames the selected task, Space completes it, and Alt+Up/Down reorders it within its list. The palette's New task command creates a linked task when a mail is selected; otherwise it opens the inline task field. Task creation Undo lasts for the existing toast window. All local task writes commit synchronously through the UI store using its atomic statements and small transactions. Failed writes keep the editor open with its text and show an error toast. Notes and optional due dates are stored by the core; this slice's inline editor changes titles only.

Still open for later slices: Google sensitive-scope verification timing; accepting the sync conflict rule; time zones and editing one recurring event versus the series.

### Critical files
- crates/lagom-core/src/oauth.rs
- crates/lagom-core/src/store.rs
- crates/lagom-core/src/schema_v9.sql
- crates/lagom-core/src/tasks.rs
- apps/desktop/src/main_window/tasks.rs
- apps/desktop/src/sync.rs
- apps/desktop/src/main_window.rs

For b1, the Google desktop OAuth, CalendarList and events.list documentation was checked on 2026-11-09. Live provider consent remains untested. Quotas, Testing-mode limits and Microsoft cross-resource refresh remain unverified. Microsoft Graph still needs a resource-keyed token cache in slice c; b1 does not call Graph. The read-only calendar cache replaces overlapping events in fetched calendars atomically, preserving other windows and calendars. Fetches run independently of mail commands; local/pending events must also be preserved when editing lands.

### Home and week view

The week starts on Monday in the system timezone. All-day dates stay in a separate strip; timed events use account accent tints and share horizontal space when they overlap. ‹ / › and the arrow keys move one week; Today or T returns to this week. The current-time line refreshes each minute. Event details show time, location and calendar, with validated HTTPS Join links and an optional Google Calendar link. Schema v11 preserves `htmlLink`; older cached events have no link until refreshed.

Schema v12 persists each account's latest successful now−7d / now+60d fetch bounds atomically with its events. Days not fully covered for every Google account show “Not synced this far”; cached events remain visible. Failed fetches keep the previous bounds. Older caches have unknown coverage until refreshed. Only the latest complete window is tracked, so gaps between fetches are never presented as covered. Repeated DST hours share the same wall-clock row. Home and Calendar reuse per-account Google consent/error controls independently of cached events. Home excludes pending moves, and stale message clicks are rejected before navigation. The sidebar header shows only the app mark and Lagom; account switching stays in Accounts. Todo reuses list creation, rename, deletion and task controls; completed/deleted tasks remain excluded. All tests use local fixtures or GPUI's headless test platform. Native layout, live consent and links in a real browser remain unverified; no GUI or screenshots were used for this change.

### Creating Google events

N, New event, or a drag on empty week-grid time opens a small sheet. Drag endpoints snap to 15 minutes; Up/Down nudges a time field by 15 minutes, carrying over midnight. Return saves and Escape discards before submission. The sheet offers the account's shown writable calendars (with an account label), a title, first/end dates, times, All day, guests, description and Add Google Meet. All-day Last day is inclusive in the sheet and exclusive on the wire. Local times carry both the RFC 3339 offset for each boundary and the system IANA zone; skipped or ambiguous DST times require correction. Settings switches and the existing sheet motion/tokens are reused. Compose's recipient widget is coupled to mail drafts, so this slice accepts comma/semicolon-separated guest addresses and reuses core splitting/validation without copying its autocomplete widget.

The worker rechecks calendar ownership and write scopes, runs creation separately from mail commands, refreshes a rejected token once, and marks 403 as needing calendar access. It upserts the returned event before notifying the window, without changing fetch coverage. A fetch started before completion is cancelled so it cannot remove the new event. Meet uses a fresh UUID createRequest, `hangoutsMeet`, and `conferenceDataVersion=1`; `sendUpdates=all` is sent only with guests. [Google events.insert documentation](https://developers.google.com/workspace/calendar/api/v3/reference/events/insert) checked on 2026-11-09. Meet creation may be pending in the initial response; the existing periodic fetch supplies the link when available. Event details already offer Join for validated HTTPS conference links.

Demo events stay in memory through week navigation and appear in Home; no invitations or real Meet links are created. `LAGOM_SCREEN=new-event` opens the demo sheet and is listed in `scripts/smoke.sh`. Creation requires a connection; ambiguous transport failures are not retried automatically, and users are asked to check their calendar before retrying. Offline queuing, recurring-event editing and shared recipient-editor extraction remain later work. Loopback HTTP and headless GPUI tests cover the create payload, DST, exclusive all-day end, token refresh, access denial, immediate cache/view updates, snapping, keyboard and cancellation. Live provider, real browser opening and native rendering remain unverified; no app/GUI/screenshots were run.
