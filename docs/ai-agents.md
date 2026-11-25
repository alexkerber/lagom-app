# AI Agents: specification and plan

Coding agents read and act on mail and calendar through the running desktop app. Security is the
main requirement. This page is the decision that the "acting tools" line in `TODO.md` waits for.
It states the threat model, the rules, the protocol and the pull requests that build it. The
desktop app ships it under Settings > AI Agents, off by default.

Scope: the desktop app on macOS and Linux. Windows follows in its own step. The mobile apps expose
nothing.

## 1. Threat model

| Actor                                                                     | Can do without this feature                                                                                                                          | Defence                                                                                                         |
| ------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| Another OS user on the machine                                            | Nothing: the data directory is 0700 and the store is 0600.                                                                                           | The socket is 0600 inside the 0700 directory. The app also checks the peer's user ID.                           |
| A malicious process of the same user                                      | Read `lagom.sqlite3` (all cached mail). Write it, which includes queueing mail: the app sends due `outbox` rows. Read token files and agent configs. | **None. Not a boundary.** The socket gives it nothing it lacks.                                                 |
| A local agent that follows instructions hidden in mail (prompt injection) | Whatever its own tools allow.                                                                                                                        | **The main case.** Levels are enforced in the app. Send needs a person. See below.                              |
| A remote agent behind an SSH forced command                               | Nothing on this machine.                                                                                                                             | **A real boundary.** It can only run `lagom remote`. Its token limits accounts, level, time and source address. |
| A stolen remote token                                                     | Nothing: the socket is not on the network.                                                                                                           | Using it needs code that runs as the user on this machine, which is row 2.                                      |

What the socket permissions do. 0600 keeps other users of the machine out. It does not separate
processes of the same user, and neither does anything else in this design. Per-agent rights
between local agents are guardrails against mistakes and prompt injection. They are not a
boundary against a hostile local process.

Full-text search uses a shared index. Response timing can reveal that a term is frequent in another account.
A process running as the same user can occupy all eight socket connections and prevent other agents from connecting.

An agent send checks authority after SMTP login on the connection that sends the message.
A revocation after that check can race the SMTP transaction for up to 60 seconds (section 9).

Keychain. Linux Secret Service and Windows Credential Manager give any process of the same user
the stored secrets. macOS asks before another program reads an item. Not verified for lagom's
release builds.

Prompt injection. Mail is text from strangers, and an agent may obey it. The design assumes that
every agent can be turned against the user, and limits what a turned agent can do through lagom:

- A Read agent has no acting command. No message it reads can change that, because the level
  belongs to its token and the app checks it on every request.
- Sending mail and changing a calendar event need a click by a person in the app. This is on by
  default. The approval covers everything that leaves, as shown.
- The socket has no command that changes levels, tokens or settings, and none that approves a
  confirmation. Only the Settings page does that.
- Agents never delete for good. Triage can move a mail to Trash. It cannot empty Trash.
- Agents get plain text, never HTML. Reading triggers no remote content.
- Write commands are rate-limited, and every call is logged.

Limits that stay:

- An agent with a shell can leave lagom: read the store with `sqlite3`, or read another agent's
  token file. Run agents in a sandbox, or give them only the MCP server, when that matters.
- A Read agent that also has network tools can leak what it reads. Give Read only for accounts the
  agent may see.
- An agent that controls the desktop (accessibility, computer use) can click Approve.
- Text extraction can keep text that the HTML hid from the human reader
  (`docs/html-to-text-known-leaks.md`).

## 2. Architecture

```
agent ──> lagom <subcommand>  ──┐
agent ──> lagom mcp (stdio)   ──┼──> unix socket ──> app: token, policy, confirm, audit ──> store / mail worker
ssh   ──> lagom remote        ──┘
```

- One binary. `lagom <subcommand>` is the desktop binary in client mode (section 12). The client
  parses arguments, forwards one request and prints the answer. It never opens the store.
- The app is the only enforcement point. It needs to run. The client says so when it does not.
- Socket: `agent.sock` beside the store (`~/Library/Application Support/lagom/` on macOS,
  `~/.local/share/lagom/` on Linux). The app binds it only while agent access is on, with mode
  0600, and removes it when access goes off or the app quits. A stale file is replaced; a live
  one means another instance serves, so the second instance does not bind. A path over the
  platform limit (104 bytes on macOS) fails with a message in Settings.
- The app never listens on the network. Remote means SSH to the machine that runs the app.
- Reads use a separate store connection with `PRAGMA query_only=ON`.
- Mail actions go to the mail worker (`apps/desktop/src/sync.rs`) as the `Act`s a click sends, so
  they keep the worker's order with sync, moves and sends. An approved send is queued the way
  Compose does it, using the Undo deadline followed by `Cmd::Send`.
- Authorization happens twice: when the request arrives, and again in the step that makes the
  change. A worker command from an agent carries the agent, the token ID and the level it needs;
  the worker reads the policy again before it acts. A command that waited in the queue through a
  revoke, an expiry or a lowered level does nothing.

## 3. Identity and tokens

Every request carries a token. No token means refused. A token belongs to one agent.

- Format: `lgm_<id>_<secret>`. `id` is 8 hex characters and public. `secret` is 32 bytes from the
  OS random source in base64url.
- The store keeps the `id` and the SHA-256 of the whole token, compared in constant time. The hash
  protects copies of the store (backups, bug reports). It does not protect the live machine,
  where the token file is readable by the same user.
- An agent has a random ID of 12 hex characters. It is never reused. It is the only agent text
  that appears in file names and generated commands.
- Local agent: Connect creates the token and writes it to `agents/<agent id>.token` (0600) in the
  data directory. The agent's config holds `--agent <agent id>`, never the token, because those
  configs end up in dotfile repositories. Local tokens do not expire. Connecting again replaces
  the token.
- Remote agent: Settings creates the token with accounts, a level, an expiry (required, 30 days
  by default, 365 at most) and a required list of source IP addresses or CIDR ranges. The app writes its token file
  too, for the forced command below, and offers Show token during setup.
- Token sources in the client: `--token <value>`, `--agent <agent id>` (reads the token file) or
  `LAGOM_TOKEN`. The first one given is the only one used. A missing token file is an error; the
  client does not fall through to another source. `--token` shows in `ps` to other users.
- Revoke deletes the agent's token rows and token file. Removing an agent also deletes the agent,
  so a forgotten `authorized_keys` line for it points at nothing.

Source address. Under a forced command, `sshd` sets `SSH_CONNECTION`. `lagom remote` sends its
first field as `source_ip`. The app refuses a remote token without `source_ip`, and one outside
the token's ranges. Local tokens are refused when a source address is present.
CIDR checks treat an IPv4-mapped IPv6 source as IPv4, including when the range is `::/0`.
The request clears its owned token buffer on drop. Limits:

- The client states the address; the app cannot check it. It holds only when the token is
  reachable through the forced command alone. A local process can state any address.
- It is the address `sshd` saw: a NAT gateway, a jump host or a VPN address.
- So `sshd` checks first. The line the app generates for `authorized_keys` carries
  `from="<ranges>"` and `expiry-time="<date>"` too:

```
restrict,from="203.0.113.0/24",expiry-time="20270101Z",command="'/Applications/lagom.app/Contents/MacOS/lagom' remote --agent 3f9a1c0b77de" ssh-ed25519 AAAA...
```

`lagom remote` reads `SSH_ORIGINAL_COMMAND`, splits it without a shell and accepts only lagom's
own agent subcommands, including `mcp`. It rejects shell characters, `--token`, `--agent`, `skill`, `remote` and
every flag that takes a file path (a path would read files of the host), in every spelling
(`--flag value`, `--flag=value`). With a forced `--agent`, only that token file is used. Without it, `lagom remote` requires `LAGOM_TOKEN`. The app shows the line above; it
never edits `~/.ssh`.

## 4. Policy

Mail levels, in order: **None < Read < Triage < Send**. Calendars have three: **None < Read <
Send**. The store keeps no guest list for an event, so the app cannot tell a private change from
one that mails guests. Every calendar change therefore needs Send.

Resources: `mail` per account and `calendar` per account. lagom has no shared-inbox account type;
a shared mailbox added as an account is a mail account. A "Shared inboxes" group is added when
the type exists.

```
effective(resource) = min( global level,   # the account's row, else the group default, else None
                           agent level,    # the same lookup in this agent's matrix
                           token scope )   # the token's level, and None for accounts not in its list
```

- Agent access off: the socket is closed and every evaluation gives None.
- Defaults are least privilege. Both group defaults start at None. A new account has no row, so
  it takes its group default. A new agent starts at Read for mail and for calendars.
- A read-only agent stays read-only: the global matrix can lower an agent, never raise it.
- The app reads the policy from the store for every check. A change in Settings and a Revoke
  apply at once. A removed account loses its rows.
- Tasks are not exposed in this version.

## 5. Commands and levels

`--json` works on every command. `--account <id or email>` narrows a command.

| Command                                                                     | Resource | Level                                                | Confirm |
| --------------------------------------------------------------------------- | -------- | ---------------------------------------------------- | ------- |
| `accounts`                                                                  | —        | lists accounts with Read or more on mail or calendar |         |
| `folders`                                                                   | mail     | Read                                                 |         |
| `emails [--filter] [--page] [--limit]`                                      | mail     | Read                                                 |         |
| `search <topic or filter> [--in <scope>] [--page] [--limit]`                | mail     | Read                                                 |         |
| `thread <id>`                                                               | mail     | Read                                                 |         |
| `attachment <id>`                                                           | mail     | Read                                                 |         |
| `contacts`                                                                  | mail     | Read                                                 |         |
| `draft list`, `draft show <id>`                                             | mail     | Read                                                 |         |
| `events`, `availability`                                                    | calendar | Read                                                 |         |
| `draft create`, `draft edit`, `draft delete`                                | mail     | Triage                                               |         |
| `action archive, move, read, unread, star, unstar, snooze, unsnooze, trash` | mail     | Triage                                               |         |
| `draft send`                                                                | mail     | Send                                                 | yes     |
| `event create, update, delete`                                              | calendar | Send                                                 | yes     |

- Scope first. A command checks the account of every input before any lookup or network call:
  IDs, `--account`, the source of a reply or forward, a target folder, a calendar. Every query
  filters by the allowed accounts in SQL, also inside `availability`, before rows are merged or
  counted.
- `--filter` is Gmail-style: `from:`, `to:`, `subject:`, `is:unread|read|starred`,
  `has:attachment`, `in:<folder>`, `before:` and `after:` (`YYYY-MM-DD`), and bare words for full
  text. The core parses it into bound SQL parameters. An unknown operator is `invalid_request`.
- `--in` takes `inbox`, `sent`, `archive`, `later`, `all` or a folder name. `--limit` is 20 by
  default and 100 at most. `--page` starts at 1. Only `emails` and `search` accept these paging flags.
- Drafts. `draft edit`, `draft delete` and `draft send` work only on a draft that the same agent
  created. Every other draft is read-only. `draft edit` names the revision it read; a draft that
  changed since gives `conflict`. The Drafts list marks a draft an agent created. An agent draft
  syncs to the account's Drafts folder on the server like any draft. List and show expose the revision only to its owner.
  Other drafts omit both the revision and update time. Within each account, drafts sort by creation time descending, then ID ascending. Drafts predating v22 share creation time zero.
- Agent drafts carry no files from disk. A forward keeps the attachments of its original, and
  needs Read on the original's account.
- `action archive`, `move` and `trash` need a server with `MOVE` or `UIDPLUS`. Without both, a
  move ends in a plain `EXPUNGE`, which also removes mail that another client marked deleted.
  The command then gives `unsupported` and changes nothing.
- `action move` takes an existing folder of the same account. Drafts and Sent roles give `forbidden`, including localized folder names.
  The store has no labels, so there is no label command. Agents cannot create a folder by name; the app still creates a missing
  Archive, Trash or Drafts folder, as it does for a person.
- `availability` is free and busy time from synced events. The sync window is 7 days back and 60
  days ahead; outside it the answer is `unsupported`.
- Not available to agents: empty Trash, delete for good, unsubscribe, account and sign-in
  commands, settings.

## 6. Protocol

One request per connection: the client connects, writes one line, reads one line and closes.
Both lines are JSON objects in UTF-8, ended by `\n`.

```json
{"v":1,"token":"lgm_…","cmd":"emails","args":{"filter":"is:unread","limit":20},"source_ip":"203.0.113.5"}
{"v":1,"ok":true,"result":{"messages":[{"id":"m1.…","account":"…","thread":"t1.…","folder":"INBOX","from":"…","subject":"…","date":"…","unread":true}],"next_page":2}}
{"v":1,"ok":false,"error":{"code":"forbidden","message":"This agent's access level does not allow this command."}}
```

- Version: `v` is 1. Another value gives `unsupported`. New optional fields may appear in
  version 1. An unknown `cmd` or argument key gives `invalid_request`.
- Sizes: request at most 1 MiB, response at most 32 MiB, attachment at most 20 MiB. A message
  body, including `draft.show`, is cut at 100 000 characters and marked `truncated`.
  Agent writes reject bodies over 100 000 characters, descriptions over 8 192 and locations over 1 024 with `invalid_request`.
  Base64 downloads have a wire budget based on 76 encoded characters per line, CRLF and up to two padding characters.
  Dense wrapping can exceed this budget even when the decoded part fits. The app also checks the actual decoded size.
  Forwarded attachments share one 20 MiB budget measured after decoding. A send accepts up to 100 recipients across To, Cc and Bcc.
  An event accepts up to 100 guests. Mail and event writes deduplicate by the whole case-insensitive address with its punycode domain, keeping the first occurrence.
  Recipients and guests with quoted or non-ASCII local parts give `invalid_request`.
  Subjects and event titles allow 998 characters. Subjects, titles and locations reject line breaks with `invalid_request`.
- Timeouts: 5 s to deliver the request and 30 s to answer. A command that waits for a person
  answers within 130 s, and the client waits that long for it.
- Concurrency: 8 connections at once, with at most 4 in-flight requests per token. Per token: 300 requests and 30 write commands a minute.
  One waiting confirmation per agent, three in total. Beyond that: `busy`.
- Every message object carries `account` and `id`, also inside thread and search results.
- Text from mail is data. Reads escape backslashes, control characters except newlines, bidi controls, tags and Unicode line and paragraph separators.
  They preserve joiners, variation selectors and soft hyphens.

Errors are fixed strings. They never carry a stack trace, a path, SQL, a server reply or data
from an account.

| Code              | Message                                                | When                                                                             |
| ----------------- | ------------------------------------------------------ | -------------------------------------------------------------------------------- |
| `unauthorized`    | Token missing, invalid, expired or revoked.            | Every token failure, also a wrong source address.                                |
| `forbidden`       | This agent's access level does not allow this command. | The object is visible to the caller, the level is too low.                       |
| `not_found`       | Not found.                                             | No such object, **or it is outside the caller's scope**.                         |
| `invalid_request` | Invalid request.                                       | Malformed line, unknown command or argument.                                     |
| `denied`          | Not approved.                                          | The person declined, or the confirmation timed out.                              |
| `conflict`        | Changed or ambiguous. Read it again.                   | The object changed since the caller read it, or an ID names two different mails. |
| `uncertain`       | The outcome is unknown. Check before you retry.        | The provider may have applied a change whose answer was lost.                    |
| `busy`            | Too many requests. Try again.                          | A limit above.                                                                   |
| `unavailable`     | Temporarily unavailable. Try again.                    | Network, sync, sign-in needed, store busy. Nothing was changed.                  |
| `unsupported`     | Not supported.                                         | Protocol version, or a command the account or server cannot do.                  |
| `too_large`       | Too large.                                             | A size limit above.                                                              |
| `internal`        | Internal error.                                        | Anything else. The detail goes to the app's log only.                            |

The client reports `app_unreachable` ("lagom is not running, or agent access is
off.") or `socket_path_too_long` ("Agent access could not start. The socket path is too long.").
An ID or account outside the caller's scope gives the same bytes as one that does not
exist, and the app decides it before any lookup. `forbidden` appears only for objects the caller
can already read. The audit log keeps the real reason.

## 7. Stable IDs

`messages.id` is a SQLite row ID and is not stable. The row is deleted and created again when a
mail moves (archive, trash, move), when a folder's UIDVALIDITY changes, when sync finds the UID
gone and when the account is added again. Agents never see it.

IDs are opaque strings. Clients must not parse them.

| Object                         | Built from                                      | Stable across                                                                |
| ------------------------------ | ----------------------------------------------- | ---------------------------------------------------------------------------- |
| Message `m1.…`                 | account ID and Message-ID header                | moves, resync, restart                                                       |
| Message `u1.…` (no Message-ID) | account ID, folder name, UIDVALIDITY, UID       | restart; ends when the mail moves                                            |
| Thread `t1.…`                  | account ID and the store's `thread_id`          | moves and resync when the mail has a Message-ID or a subject; one store only |
| Draft `d1.…`                   | account ID and the draft's own ID               | until sent or deleted                                                        |
| Event `e1.…`                   | account ID, calendar remote ID, event remote ID | sync                                                                         |
| Attachment `a1.…`              | message ID and MIME section                     | as its message                                                               |

- Long message headers, folder names, thread keys and attachment sections use compact SHA-256 forms.
  Lookup matches the digest inside the authorized account or message.
- Each is the prefix plus base64url of the parts. The migration adds an index on
  `messages(account, message_id)` for the lookup.
- Resolving: decode, then require Read on the account inside the ID, else `not_found`. Then query
  with `WHERE account = <that account>`. An ID is a name, not a permission: a forged ID for
  another account ends at the first check.
- Copies. One mail can sit in several folders of one account, for example Gmail's Inbox and All
  Mail. Rows with one Message-ID and the same date, size, sender and subject are copies. A
  command takes the first copy in this order: Inbox, other folders, Sent, Archive, Junk, Trash.
  `emails` and `search` page over distinct IDs per account. Each row names the preferred matching
  folder and lists all its folders in `folders`. `--folder` picks another.
- Collisions. The sender chooses the Message-ID, so a stranger can reuse one. When the rows for
  an ID are not all copies, or two of them share a folder, every write gives `conflict` and a
  read marks the object `"ambiguous":true`. What stays: a forgery with equal date, size, sender
  and subject in another folder counts as a copy.

## 8. Read purity

A read command changes nothing a person can see: not read or star state, folder, Later time,
drafts, outbox or events. Caches may fill.

| Path today                                                                                                        | What it changes                                                               | Rule for agents                                                                |
| ----------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| `Cmd::MarkSeen` → `ImapSync::mark_seen`, sent when a person opens a mail                                          | `\Seen` on the server, `messages.seen`                                        | Never sent by a read.                                                          |
| `Cmd::Inline` → `fetch_inline`                                                                                    | Loads inline images; fetches the whole message again when its HTML is missing | Never. Agents get the cached text; a missing body is `"body_available":false`. |
| `Cmd::OpenMessage` → `cache_eml`                                                                                  | Writes an `.eml` file                                                         | Never.                                                                         |
| `Cmd::Download` → `save_download`                                                                                 | Writes a file to Downloads, raises a UI event                                 | Never. `attachment` uses a new worker command that returns the bytes.          |
| `ImapSync::fetch_attachment`                                                                                      | `BODY.PEEK` only; fills `attachment_cache`                                    | Allowed: a cache.                                                              |
| `Store::message`, `thread_messages`, `search_accounts`, `mailbox_messages`, `folders`, `contacts`, `today_events` | Nothing: `SELECT` only                                                        | The read commands use these.                                                   |

Two things hold it in place. The read connection is `query_only`, so SQLite refuses a write. The
dispatcher has one list of read commands, and those can send only the attachment command to the
worker.

## 9. Confirmation pop-ups

Every calendar change requires approval in the app. Mail sends also require approval by default.
The Confirmation toggle in Settings controls mail sends for all agents.

1. The app checks token and level, then snapshots what would leave. Mail: the `Outgoing` built
   from the draft, with the draft's revision. Event: the new values, and for a change or a
   delete the event as Google has it now, with its guests and its ETag.
2. A sheet opens in the main window, in the app's own style, not a system dialog. If the window
   is not in front, a system notification says that an agent waits. The notification has no
   Approve button.
3. Fixed rows show the agent, the account and each recipient or guest. A mail labels the account
   From. Addresses appear first, one per row. Display names show up to 64 characters followed by the exact number omitted.
   Internationalized domains use punycode with a warning mark.
   The sheet escapes controls, Unicode line separators and all Unicode format and default-ignorable characters.
   A literal backslash appears doubled.
   Long subjects and titles wrap. Calendars appear by display name.
   Timed events show local start and end with the weekday and UTC offset. All-day events show calendar dates.
   An event change compares with Google's current event. Each guest appears once. A guest the
   change adds is marked Added, and one it removes is marked Removed. The comparison uses the
   whole address: punycode domain, any case, no display name. An unchanged field shows once. A
   changed field shows one row with the current value struck through, an arrow and the new value.
   The sheet compares raw descriptions and shows the current description when they differ. Body and description appear
   last in scrollable boxes with the complete reviewed text. Expand appears only on overflow.
   Each text box and the outer preview show “More below” while content remains below their viewport. Replies and forwards send only the reviewed plain text and listed attachments.
4. **Deny** has focus and Escape declines. **Deny and revoke** also revokes the agent's tokens.
   After either denial or a disconnect while waiting, the same agent receives `busy` for 30 seconds.
   Approval and revocation require a mouse click in the active window. Each front sheet and window activation
   starts a 700 ms delay. A replacement for a withdrawn request says “The previous request was withdrawn”
   and waits 1.5 seconds. The cooldown starts at withdrawal even with the window closed. Only the replacement shows the notice.
   Activation clicks and keyboard shortcuts cannot approve or revoke.
5. While the sheet awaits approval, a decline, a 120 s timeout or a client disconnect ends the request.
   The audit records `denied` and the app sends nothing.
6. On Approve the app authorizes again (token, expiry, levels, master switch) and compares the
   snapshot: the draft's revision, or the event's ETag as a precondition on the write. A change
   since step 1 gives `conflict`, sends nothing and keeps the newer draft. Otherwise the snapshot
   goes out the way Compose sends, with its Undo window. After Approve the send continues even
   when the client is gone; `draft show` then gives `not_found`.

The worker queues the send only if the policy generation still matches the approval snapshot inside the enqueue transaction.
On the authenticated connection, immediately before MAIL FROM, the outbox checks the recorded policy generation, token and effective Send access.
Revocation after this check cannot recall the transaction. `SUBMIT_DEADLINE` bounds the wait for acceptance to 60 seconds.
Disabling access or changing a grant invalidates queued approvals even if access is later restored.
Only agent-initiated sends carry the agent and token IDs. A person can send an agent draft from Compose.
Revocation, Disconnect, token replacement and disabling access commit the authority change before restoring drafts.
Grant and token changes sweep all stale unclaimed sends, including failed sends. Startup recovery repeats the sweep.
A worker releasing a claim after a definite failure restores stale mail in the same transaction.
Uncertain deliveries stay available for the person to review. Each restoration has its own savepoint.
A failed restoration rolls back that draft, logs the failure and drops its queue row.
Undo and revocation keep any newer Compose draft. Successful revocation cleanup audits `revoked`.
Sending from Compose replaces an unclaimed, unsent agent message and audits `superseded` in the enqueue transaction.
Reconnecting cannot release an old send. A message already submitted to SMTP stays submitted.

The agent cannot approve: the protocol has no such command. Confirmations queue one at a time. A
remote agent needs a person at the app for calendar changes and for mail sends with confirmations enabled.

## 10. Audit log

Every request that reaches the app is logged, refused ones too. Table `agent_audit`:

| Column           | Content                                                                                                      |
| ---------------- | ------------------------------------------------------------------------------------------------------------ |
| `at`             | Unix seconds                                                                                                 |
| `agent`, `token` | Verified agent ID and public token ID, including expiry and source refusals. Empty until the digest matches. |
| `account`        | The account the command touched                                                                              |
| `command`        | For example `emails`, `draft.send`                                                                           |
| `target`         | Canonical ID from an in-scope store lookup. NULL for unresolved or out-of-scope targets.                     |
| `result`         | `ok` or the error code, with the real reason for `unauthorized` and `not_found`                              |
| `source_ip`      | As stated by the client                                                                                      |
| `detail`         | Recipients of a send or an event. Nothing else.                                                              |

- A dispatched write reserves a durable `pending` row. A local change and its final audit result commit together.
  Remote writes finalize the reserved row even after a client disconnect. A lost mutation reply gives `uncertain`.
  A new listener marks interrupted pending writes uncertain. A confirmed calendar update stays successful if cache refresh fails.
  On UIDPLUS-only servers the worker rechecks authorization after COPY. Losing access leaves the copy and gives `uncertain`.
  Losing access after STORE marks the original deleted also gives `uncertain`. The original may retain its deleted flag.
  Restoring the flag after a refused UID EXPUNGE requires fresh authorization.
  A lost CREATE reply or failure to rediscover an acknowledged folder gives `uncertain`.
- A send uses the draft ID, shared with the outbox row and the sent Message-ID.
- Never logged: subjects, bodies, filter and search text, attachment bytes, tokens.
- The decoder rejects IDs longer than 2 KiB before decoding.
- All rows expire after 90 days. The app prunes at startup and once a day during use.
- Each append keeps the latest 50 000 non-write rows per agent. Anonymous refusals have their own bucket.
  Draft changes, sends, mail actions and event changes expire only by age.
- Refusals without a verified digest are limited to 60 rows a minute, then counted in one row.
- Verified refusals before worker dispatch and busy replies share one counter per agent per minute. The first refusal records its reason.
- Socket admission failures are counted off the listener thread. Settings shows the newest 50 rows for each agent.
- Rows have no foreign keys: they outlive the agent and the token. Removing an account deletes its rows.
- Settings shows the log by agent, newest first, with Revoke on each agent.

## 11. Store

Migration `schema_v22.sql`:

```sql
CREATE TABLE agents (id TEXT PRIMARY KEY, kind TEXT NOT NULL, name TEXT NOT NULL,
 host TEXT NOT NULL, remote INTEGER NOT NULL DEFAULT 0, created INTEGER NOT NULL);
CREATE TABLE agent_tokens (id TEXT PRIMARY KEY, agent TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
 hash BLOB NOT NULL, level INTEGER NOT NULL, accounts TEXT, cidr TEXT, expires INTEGER,
 created INTEGER NOT NULL, last_used INTEGER);
-- agent '' is the global matrix. account '' is the group default. kind is 'mail' or 'calendar'.
CREATE TABLE agent_access (agent TEXT NOT NULL, kind TEXT NOT NULL, account TEXT NOT NULL,
 level INTEGER NOT NULL, PRIMARY KEY(agent, kind, account));
CREATE TABLE agent_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL); -- enabled, confirm, sidebar
CREATE TABLE agent_drafts (draft TEXT PRIMARY KEY, agent TEXT NOT NULL); -- kept until the draft is gone
CREATE TABLE agent_audit (id INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL, agent TEXT, token TEXT,
 account TEXT, command TEXT NOT NULL, target TEXT, result TEXT NOT NULL, source_ip TEXT, detail TEXT);
CREATE INDEX agent_audit_at ON agent_audit(at);
CREATE INDEX messages_account_message_id ON messages(account, message_id);
```

Migration `schema_v22.sql` also indexes counted read rows by agent and sequence.
An agent send stores its owner in `outbox.draft_agent` and the originating token ID in `outbox.agent_token`.
Deleting that token clears the reference, including on claimed sends. Undo restores the agent's draft in one transaction.
Compose leaves both fields empty, including when the person sends an agent-created draft.

`Store::delete_account` also deletes the account's `agent_access` rows.

## 12. The `lagom` command, MCP and `lagom-cli`

- The desktop crate's binary is already named `lagom`, so a second binary of that name cannot
  exist in the workspace. `main()` checks `argv[1]` before GPUI starts. A known subcommand,
  `--help` or `--version` runs the client and exits. Anything else starts the app as today, also
  `mailto:` links.
- macOS: Connect, or a button in Settings, links `~/.local/bin/lagom` to the app's binary. It
  replaces only a link it made itself, and says so when that directory is not on `PATH`. No link
  is made unless the app runs from an Applications folder.
- Linux: the package installs `/usr/bin/lagom`, so the command exists already.
- Windows: later (plan, step g). The release binary has no console and the transport is a named
  pipe for the current user.
- `lagom mcp` is an MCP server over stdio. Each tool call becomes one socket request with the
  caller's token, so the rights are the same. `tools/list` shows a tool when the token reaches
  its level on at least one resource; the app still checks every call. Tools: `list_accounts`,
  `list_folders`, `list_mail`, `search_mail`, `read_thread`, `get_attachment`, `list_contacts`,
  `list_events`, `find_availability`, `list_drafts`, `read_draft`, `create_draft`, `edit_draft`,
  `delete_draft`, `send_draft`, `mail_action`, `create_event`, `update_event`, `delete_event`.
  Each maps to the command of section 5 with the same level.
- `lagom skill install [--for <kind>] [--dir <path>]` writes the bundled `SKILL.md`. It is a
  file write by the client and creates no token.
- `lagom-cli` stays a developer test client with the user's full rights: it opens the store,
  reads the keychain and talks to the mail servers itself. It skips every rule here. It is not
  packaged, not on `PATH` and never named in a skill or an MCP entry. Use the desktop binary for MCP. Tasks remain available through the developer CLI.

## 13. Agent detection and Connect

Detection reads the file system only: a known binary on `PATH` or a known config directory.

| Agent       | Detected by                            | Skill written to                  | MCP entry                     |
| ----------- | -------------------------------------- | --------------------------------- | ----------------------------- |
| Claude Code | `claude`, `~/.claude/`                 | `~/.claude/skills/lagom/SKILL.md` | `claude mcp add --scope user` |
| Codex       | `codex`, `~/.codex/`                   | `~/.codex/skills/lagom/SKILL.md`  | `codex mcp add`               |
| Cursor      | `cursor`, `cursor-agent`, `~/.cursor/` | none                              | `~/.cursor/mcp.json`          |
| Hermes      | `hermes`, `~/.hermes/`                 | `~/.hermes/skills/lagom/SKILL.md` | none; uses the skill          |
| OpenClaw    | `openclaw`, `~/.openclaw/`             | its skills directory              | none; uses the skill          |
| Other       | added by hand                          | a directory the person picks      | a snippet to copy             |

Each row shows the agent's mark (sources in `apps/desktop/assets/logos/README.md`) or a
monogram, its name and one line under it:

- Not connected: "Installed" and a Connect button.
- Connected: an Active pill, what it may do on any account under both matrices (for example
  "Triage mail · Read calendar") and its last use, with Disconnect.
- Connected while agent access is off: a Paused pill and "Agent access is off". Every request
  fails in this state.
- Remote: the host label, "Remote over SSH", the expiry date and Revoke or Remove.

The binaries and directories above were seen on one development machine. The
skill directories of Cursor and OpenClaw and the exact arguments of each `mcp add` are not
verified; check them in the pull request.

Connect writes only after a click, and first lists every file it will write:

1. The token file in the data directory.
2. The skill file. Its examples use `lagom --agent <agent id>` and name the binary's absolute
   path for the case that `lagom` is not on `PATH`.
3. The MCP entry `lagom mcp --agent <agent id>`, with the absolute path of the binary, because
   agents started from the desktop do not get the shell's `PATH`. It goes through the agent's
   own command where one exists. `~/.cursor/mcp.json` is parsed, changed and written back
   atomically with its other keys kept. A file that does not parse is left alone.

Disconnect on an agent revokes its token, removes the MCP entry and removes the skill file when
it is unchanged. Disconnect in the header does this for every agent and switches access off.

## 14. Settings > AI Agents

- Header: title, a link to this page on the site, Disconnect.
- "Enable agent access", off by default, with the warning: use trusted agents in an environment
  you control; an agent can be misled by the mail it reads.
- Agent list (section 13) and "Add remote agent".
- Access: one block per group with its default level, then a row per account. A mail row has
  four segments (None, Read, Triage, Send); a calendar row has three (None, Read, Send). Opening
  an agent shows the same matrix for that agent. A segment above the global level is disabled.
- "Confirmation pop-ups", on by default.
- "Show AI Agents in sidebar": a row with the number of active agents and a mark while a
  confirmation waits. It opens this page.
- Activity: the audit log with Revoke.
- No paid tiers.

## 15. Plan

Each step is one pull request, mergeable and testable alone. Until step e nothing can switch
access on in a release build. Manual tests before that use a `lagom-cli agent` developer command
that writes the rows (step b).

| PR  | Content                                                                                              | Files                                                                                                                                                                                                                                |
| --- | ---------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| a   | Policy, tokens, ID codec, audit, protocol types, migration 22                                        | `crates/lagom-core/src/agent.rs`, `agent/*.rs`, `schema_v22.sql`, `store.rs`, `lib.rs`                                                                                                                                               |
| b   | Socket server, dispatcher, read commands, filter parser, developer command                           | `apps/desktop/src/agent.rs`, `agent/read.rs`, `sync.rs` (attachment command), `main_window/live.rs` (start and stop), `Cargo.toml` (tokio `net`, `io-util`), `crates/lagom-core/src/agent/filter.rs`, `crates/lagom-cli/src/main.rs` |
| c   | `lagom` client, `--json`, `skill install`, `mcp`                                                     | `apps/desktop/src/main.rs`, `cli.rs`, `cli/mcp.rs`, `cli/skill.rs`, `assets/skill/SKILL.md`                                                                                                                                          |
| d   | Draft and action commands, worker authorization, confirmation sheet, notification                    | `apps/desktop/src/agent/write.rs`, `agent/confirm.rs`, `sync.rs`, `main_window.rs`, `main_window/drafts.rs`, `notifications.rs`                                                                                                      |
| d2  | Event commands; Google read with guests and ETag, update and delete in the core (only create exists) | `crates/lagom-core/src/google/create.rs`, `calendar.rs`, `apps/desktop/src/agent/write.rs`, `sync.rs`                                                                                                                                |
| e   | Settings page, sidebar row, detection, Connect and Disconnect, audit view; remove `lagom-cli mcp`    | `apps/desktop/src/settings.rs`, `settings/agents.rs`, `agent/detect.rs`, `main_window.rs` (sidebar), `crates/lagom-cli/src/{main,mcp}.rs`, `README.md`                                                                               |
| f   | Remote tokens in Settings, `lagom remote`, SSH guide                                                 | `apps/desktop/src/cli/remote.rs`, `settings/agents.rs`, `docs/agents-ssh.md`                                                                                                                                                         |
| g   | Windows: named pipe, console output, `PATH` in the installer                                         | `apps/desktop/src/agent.rs`, `cli.rs`, `scripts/package-windows.iss`                                                                                                                                                                 |

Order: a, b, c, d, d2, e, f, g. Step e can start after b. Step c reports `binary.size` against
`docs/performance-budgets.toml`, because the client adds an argument parser to the app binary.

Tests that prove the required properties:

| Property                     | PR   | Test                                                                                                                                                                                                                                                                                                                                                                                                |
| ---------------------------- | ---- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Enforcement per account      | a    | `effective()` over the matrix: own row, group default, no row at all, token account list.                                                                                                                                                                                                                                                                                                           |
|                              | b    | Token for account A: lists, search and `availability` hold no row of B; `--account B` is `not_found`.                                                                                                                                                                                                                                                                                               |
| Enforcement per agent        | a    | Global Triage and agent Read give Read. The global matrix never raises an agent.                                                                                                                                                                                                                                                                                                                    |
|                              | d    | Over the socket: the Read agent's `action archive` is `forbidden` and the mail stays; the Triage agent's succeeds. One agent cannot edit, delete or send another agent's draft, or a person's.                                                                                                                                                                                                      |
| Authorization at the change  | d    | A write queued behind a paused worker does nothing after a revoke, an expiry or a lowered level.                                                                                                                                                                                                                                                                                                    |
| Access off                   | b    | No socket file while off. Switching off closes it and ends open requests.                                                                                                                                                                                                                                                                                                                           |
| No state change on read      | b    | Every read command runs against a seeded store and a scripted IMAP server. A digest of messages (flags, folder, Later), drafts, outbox and events is equal before and after. The server sees no `STORE`, and fetches only with `BODY.PEEK`. A write on the read connection fails.                                                                                                                   |
| Cross-account IDs rejected   | a    | Codec round trip. An ID of account B resolves to `not_found` for a caller with A only. Rows that are not copies give `conflict` on a write.                                                                                                                                                                                                                                                         |
|                              | b    | The response for a B ID is byte-equal to the response for an ID that does not exist, and no query runs for it.                                                                                                                                                                                                                                                                                      |
|                              | d    | `move` to a folder of B, a draft of B and a forward of a B mail are `not_found`. Nothing changes.                                                                                                                                                                                                                                                                                                   |
| Confirmation blocks sends    | d    | `draft send` waits. Decline, timeout and client disconnect give `denied` and an empty outbox. A revoke, an expiry or a lowered level while waiting sends nothing. A draft edited or deleted while waiting gives `conflict` and the newer draft stays. Approve queues exactly the snapshot, and Undo brings the draft back. No protocol command approves. With the toggle off the send goes through. |
|                              | d2   | Create, update and delete wait for the sheet and need Send. The sheet lists guests that the request did not name. An event changed at Google while waiting gives `conflict`. A lost answer gives `uncertain`. A calendar of B is `not_found`.                                                                                                                                                       |
| No delete for good           | d    | A server without `MOVE` and `UIDPLUS`: `archive`, `move` and `trash` give `unsupported` and the scripted server sees no `EXPUNGE`.                                                                                                                                                                                                                                                                  |
| Token expiry                 | a    | Expired, revoked, malformed and unknown tokens each give `unauthorized`.                                                                                                                                                                                                                                                                                                                            |
| IP limits                    | a    | IPv4 and IPv6 ranges; an address outside; a remote token without `source_ip`.                                                                                                                                                                                                                                                                                                                       |
|                              | f    | `lagom remote` with `SSH_CONNECTION` and `SSH_ORIGINAL_COMMAND` set: inside and outside the range. Shell characters, `--token` and `--agent` in both spellings, path flags and unknown subcommands are refused. A missing token file fails with `LAGOM_TOKEN` set. The line for `authorized_keys` parses back to the same agent, ranges and expiry. A removed agent's line reaches no newer agent.  |
| Fixed errors                 | b    | Every error line matches the table in section 6. A forced internal error carries no path or SQL.                                                                                                                                                                                                                                                                                                    |
| Audit                        | b, d | Each call, also a refused one, adds one row. A filter that holds a subject and a token leaves neither in the log. Revoke stops the next request.                                                                                                                                                                                                                                                    |
| Connect writes on click only | e    | With a temporary home: detection writes nothing; Connect writes only the listed files, the token file is 0600, and a `lagom` on `PATH` that the app did not link stays. The MCP entry starts with the agent's own identity. Disconnect removes what Connect wrote.                                                                                                                                  |

## 16. Deferred and open

Decided, and left out of this version:

- `draft schedule` returns `unsupported`. The app has no list of scheduled mail, so a person
  could not see or cancel what an agent scheduled. It follows that list.
- Tasks, shared inboxes, files from disk in agent drafts, Windows.

To verify during the work, not decisions:

- Whether macOS asks before another program reads lagom's keychain items in a release build.
- The skill directories and MCP commands in section 13.
- Whether `clap` keeps the app binary inside its size budget; if not, the client parses by hand.

Open for the maintainer: nothing blocks step a.
