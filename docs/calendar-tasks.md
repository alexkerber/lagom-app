# Spec: Calendar and Tasks for Lagom

Based on `feat/oauth` at 0309143. Anything I could not check against live docs is marked **not verified**.

## Three things in the current code that shape the design
1. **The token code assumes one scope string per provider.** `oauth.rs` caches one token per account id (`cache(&account.id)`), and on refresh it sends the fixed `config.scope` again.
   - Google: once accounts hold different scope sets, sending scopes the user never granted may break the refresh (**not verified**). Fix: drop `scope` from Google refreshes, which keeps what was granted.
   - Microsoft: a token covers one resource. The IMAP token (`outlook.office.com`) cannot call Graph. Fix: `access_token(account, Resource::{Mail, Graph}, rejected)`, with the cache keyed by `(id, resource)`. Microsoft's refresh token can usually be exchanged for a Graph token after consent (**not verified** for personal accounts).
2. **A mail→task link cannot use `messages.id`.** A move deletes the row and inserts a new one, and Done is a move. Link by `(account, Message-ID)`, which is what the worker already uses to follow moved mail (`sync.rs:422-440`). Also keep a copy of the subject, so the link still makes sense once the mail is gone. Mail with an empty Message-ID gets a task with no link.
3. **Key `t` is free** in `main_window.rs:34-55`.

## 1. Provider APIs and scopes
| | Calendar | Tasks |
|---|---|---|
| Google | Calendar v3: `calendarList.list`, `events.list?singleEvents=true&timeMin&timeMax`, `events.insert?conferenceDataVersion=1&sendUpdates=all` | Tasks v1: `tasklists.list`, `tasks.list?showDeleted&showHidden&updatedMin` |
| Scopes | `calendar.events` + `calendar.calendarlist.readonly` (the second one is **not verified** to exist), or else `calendar` | `tasks` |
| Microsoft Graph | `/me/calendars`, `/me/calendarView?startDateTime&endDateTime`, `POST /me/events` | `/me/todo/lists`, `/me/todo/lists/{id}/tasks` (a delta endpoint exists) |
| Scopes | `Calendars.ReadWrite offline_access` | `Tasks.ReadWrite` |

**Google consent**
- `https://mail.google.com/` is a restricted scope, so production already needs verification plus a CASA assessment.
- The Calendar and Tasks scopes are only sensitive. They add a justification and a demo video, not another CASA.
- While the app is in Testing mode, refresh tokens expire after 7 days and there is a cap of 100 test users (**not verified** for 2026).
- Users who are already signed in have none of these scopes. Use incremental authorization: when someone first opens the agenda or turns on task sync, run the existing loopback flow for the new scopes only, with `include_granted_scopes=true&login_hint=<email>`. Reject the result if the ID-token email does not match the account.
- Google shows granular consent checkboxes, so read the `scope` field in the token response and store what was actually granted.

**Microsoft:** these are delegated scopes a user can consent to alone. Some tenants block user consent (**not verified** per tenant).

**CalDAV / iCloud: leave out of v1.** It needs our own iCalendar parsing and RRULE expansion, iCloud Reminders no longer goes through CalDAV, and server-side expansion support on iCloud is **not verified**.

**Quotas: not verified.** Rough figures: Calendar about 1M requests/day per project, Tasks about 50k/day, Graph throttling around 10k requests per 10 min per mailbox.

## 2. Store: `schema_v4.sql`
Same style as `schema_v3.sql`: change the guard in `Store::open` to `version > 4`, add `if version < 4`, and add a v3→v4 test like `store.rs:163`.
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

New core modules: `crates/lagom-core/src/tasks.rs` and `calendar.rs` (types and store queries, plain `String`/`i64`, ready for UniFFI later), plus `google.rs` and `graph.rs` (JSON over the existing `reqwest`/`serde_json`). No new dependencies.

## 3. Sync model
- **Calendar:** on each worker tick (the existing 2-minute one), re-fetch now−7d to now+60d per shown calendar and replace that window in one transaction. This needs no syncToken or deltaLink and has no state to corrupt. Mark it `shortcut: refetch window, move to syncToken/delta when request counts matter`.
- **Tasks, order of work:** push first, then pull.
  - Push: send rows with `dirty=1` or `deleted=1` as PATCH, DELETE or INSERT, then clear the flags.
  - Pull: Google with `updatedMin` (it has no sync token); Graph by refetching the list (delta later).
- **Conflict rule:** last writer wins, and the local edit wins because it is pushed before the pull. The cost is losing a server edit made in the same 2-minute window. Alex should accept this.
- **Offline:** the `dirty`/`deleted`/`pending` columns are the queue. There is no separate ops table.
- **Retries without duplicates:**
  - Google `events.insert` accepts a client-made `id` (base32hex).
  - Graph events take a `transactionId`.
  - Google Tasks has no idempotency key, so a retry after a timeout can create duplicates (**not verified** whether there is a workaround).
- **Mail link when synced:** Graph uses `linkedResources` (`externalId` = Message-ID). Google Tasks `links` is read-only, so add a line to `notes` instead.

## 4. Desktop slices
Each slice ships on its own. New `Cmd`s go in `apps/desktop/src/sync.rs`, the UI keeps reading the store, and `Event::Changed` is reused.

| Slice | Work | Size |
|---|---|---|
| a. Local tasks + `t` | `tasks.rs` view on the start screen. `t` creates a task in the default list with title = subject, plus the Undo toast. Add/rename lists. Opening a task's link finds the mail by Message-ID. No network. | M |
| b. Google agenda, read-only | Incremental-consent flow, `Resource` in `oauth.rs`, `google.rs` window fetch, `agenda.rs` column next to the inbox. Then Google Tasks sync. | M |
| c. Microsoft | Graph token, `graph.rs` calendarView and To Do. | M |
| d. Create/edit events | Form with `conferenceData.createRequest{requestId, conferenceSolutionKey:{type:"hangoutsMeet"}}`. On Graph, `isOnlineMeeting` (Teams only for work accounts, **not verified** for personal). | L |
| d′. Zoom | Needs its own OAuth app in the Zoom Marketplace, its own review, and possibly a client secret in a desktop app (**not verified**; PKCE support **not verified**). The join URL goes into location/description. | L, separate |

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
  - v3→v4 migration seeds Work/Private
  - removing an account keeps local tasks
- Desktop: a `TestAppContext` test where `t` creates a task (same pattern as `main_window.rs:2372`).

## 6. Decisions for Alex
1. **What is the "start screen"?** The empty-inbox view, the sidebar, or a new home view?
2. **Work/Private vs the provider's lists.** Should they map onto them (e.g. Google "My Tasks" = Private), or should remote lists appear as their own lists under the account?
3. **Which list `t` uses**, and whether it asks first.
4. **When to ask for scopes.** All scopes once at slice b (one re-consent) or one feature at a time (several prompts)?
5. **Google verification.** When to submit for sensitive-scope verification. In Testing mode the 7-day expiry means beta users sign in again every week.
6. **Zoom in v1:** yes or no.
7. **Time zones and RRULE editing.** Editing "this event vs the whole series" is part of the L size in slice d.

### Critical files
- /Users/alex/Developer/lagom-app-worktrees/oauth/crates/lagom-core/src/oauth.rs
- /Users/alex/Developer/lagom-app-worktrees/oauth/crates/lagom-core/src/store.rs
- /Users/alex/Developer/lagom-app-worktrees/oauth/crates/lagom-core/src/schema_v3.sql (pattern for the new schema_v4.sql)
- /Users/alex/Developer/lagom-app-worktrees/oauth/apps/desktop/src/sync.rs
- /Users/alex/Developer/lagom-app-worktrees/oauth/apps/desktop/src/main_window.rs

I did not check any provider docs live: scope names, quotas, Testing-mode limits, Zoom PKCE support and Microsoft refreshing across resources all come from memory. The biggest risk is the shared token cache and the fixed refresh scope in `oauth.rs`, which break as soon as a second scope set or resource is added.
