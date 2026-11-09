# Lagom mail core

Local-first Rust API shared by the desktop app and CLI.
`Store` exposes plain structs and reads SQLite only. Run network operations on
an executor outside the UI thread; own each `Store` on its worker thread.

`async-imap` supplies CONDSTORE and IDLE without a blocking thread per account.
Transport is rustls with certificate/hostname verification and required STARTTLS;
`Plain` is rejected outside loopback. Protocol trace logging is compiled out
because async-imap's wire trace includes LOGIN credentials. Lettre's optional
tracing and connection pool features are disabled. Passwords go only to OS
keychain entries with service `lagom` and account UUID as key.

## Tests

Unit tests need no network or keychain; credential tests use keyring's mock.
Integration tests use unique disposable GreenMail users and temporary databases.
They are ignored by default; `LAGOM_GREENMAIL=1` enables them at compile time.

```sh
docker run -d --rm -p 127.0.0.1:3143:3143 -p 127.0.0.1:3025:3025 \
  -e 'GREENMAIL_OPTS=-Dgreenmail.setup.test.all -Dgreenmail.hostname=0.0.0.0 -Dgreenmail.users=test:test@lagom.test -Dgreenmail.auth.disabled' \
  greenmail/standalone:2.1.2
# Save the container id printed above.
LAGOM_GREENMAIL=1 cargo test -p lagom-core --test greenmail
# Stop only that container, using its saved id:
docker stop <container-id>
```

Coverage: login, folders, SMTP delivery then INBOX sync, Sent APPEND, seen/unseen
and external flags, bounded initial sync, incremental arrivals, local snooze,
folder creation, MOVE/archive/trash, external expunge, HTML/charset parsing,
attachment detection without fetching attachment bodies, IDLE notification, and
COPY + UID EXPUNGE fallback without expunging another client's deleted mail.
A local scripted IMAP peer also tests CONDSTORE, SPECIAL-USE and rejection of a
stale UID operation after a UIDVALIDITY change.

## Failure and sync semantics

Drop/reconnect an IMAP session after any network/protocol error or cancellation;
partially consumed commands must not be reused. No automatic retry or polling.
A failed operation may have reached the server; resync before retrying a move.
`Error::SentCopy` means SMTP succeeded: do not submit the message again.

Draft autosave waits for a 300 ms typing pause and writes on a background executor.
Revision checks prevent a late autosave from overwriting a newer save or restoring
sent/discarded mail. Explicit Send, close and quit still flush local drafts synchronously.
Sending commits an outbox row before the five-second Undo window; Undo atomically
restores the draft. Temporary failures retry on sync with exponential backoff from
one minute to one hour. Permanent refusals stop automatic retry. Interrupted SMTP
attempts and unknown delivery outcomes remain uncertain until the user chooses
Send again (which can duplicate delivery) or Edit draft (which atomically cancels
the queued entry). Failed send status and actions survive restarting the desktop.
SMTP and acknowledged Sent APPEND are checkpointed separately before refreshing
the local Sent cache. A lost APPEND acknowledgement or crash before its checkpoint
can still duplicate a Sent copy; Message-ID stays unchanged. The synchronous CLI
send API leaves no queued entries on failure. Quit does not wait for network work.
One desktop worker owns the queue per store. Server-side Drafts sync is not implemented.

Initial sync stores the newest 500 UIDs per folder by default; subsequent sync
keeps cached older messages and fetches every UID beyond the saved checkpoint.
UIDVALIDITY changes invalidate that folder's cached rows. Successful sync compares
all server UIDs to detect expunges. CONDSTORE refreshes changed flags across cached
UIDs; other servers refresh flags for the newest N. Snooze uses caller-supplied
Unix timestamps, with no background timer in the core.

The first preferred plain/HTML text body part is cached. Multiple inline text parts are not combined yet.
Selected encoded text plus its headers must fit 4 MiB (part headers <64 KiB). Larger text bodies need streaming support.
Server-deleted folders and their cached messages are removed after a successful complete LIST.

TLS/STARTTLS against real providers and the real OS keychain require manual
account testing; GreenMail's integration ports exercise local plaintext only.

## Local queries and search budget

`list_messages` returns metadata in date/id descending order with limit/offset;
`message` loads the full cached text/optional HTML. `folders` includes unread
counts; `counts` exposes Inbox/Later totals; `later` lists future snoozes. Due
snoozes become visible at query time. Search accepts FTS5 expressions across
subject, sender name/address and body, with stable row-id pagination. It returns
index order, avoiding a relevance sort over every match.

```sh
cargo test -p lagom-core --release search_100k_budget -- --ignored --nocapture
```

Measured 2026-11-06 on this Apple silicon host, 100,000 synthetic messages and
50 first-page results: unique body token 0.205 ms, common body token 0.160 ms,
sender 0.111 ms, subject 0.101 ms. All passed the <50 ms assertion. This is one
local run with a freshly populated database in the OS cache, not a cold-disk,
real-mailbox or CI performance guarantee. UI startup, frame timing, memory and
idle CPU budgets remain unmeasured by this work.

## CLI

```sh
cargo run -p lagom-cli -- sign-in google       # or microsoft
cargo run -p lagom-cli -- discover you@example.com
cargo run -p lagom-cli -- add-account you@icloud.com
cargo run -p lagom-cli -- accounts
cargo run -p lagom-cli -- sync                 # all accounts; or sync <UUID/email>
cargo run -p lagom-cli -- folders
cargo run -p lagom-cli -- list INBOX --limit 20
cargo run -p lagom-cli -- show <message-id>    # local numeric id printed by list
cargo run -p lagom-cli -- search 'subject:invoice'
cargo run -p lagom-cli -- send --to recipient@example.com --subject Hi --body Hello
cargo run -p lagom-cli -- archive <message-id>
cargo run -p lagom-cli -- trash <message-id>
cargo run -p lagom-cli -- move <message-id> Receipts
```

For multiple accounts, pass global `--account <UUID/email>`. Folder selectors
accept an exact name or local numeric id. `--database <path>` selects a test
store. `folders`, `list`, `show` and `search` perform no network/keychain access.
`send` also accepts comma-separated `--cc`/`--bcc`, `--in-reply-to` and repeated
`--references`. Bcc recipients enter the SMTP envelope only, never the saved header.

`add-account` verifies IMAP login, then saves the password to the OS keychain and
account metadata to SQLite. It reads a hidden terminal password interactively,
or one line from stdin when piped. No password argument or environment variable
is accepted. Unknown domains require both server overrides. Standard overrides
use `--imap host:port --smtp host:port`; custom transport modes use
`--imap-security tls|start-tls|plain --smtp-security tls|start-tls|plain`.

For a disposable GreenMail account:

```sh
cargo run -p lagom-cli -- --database /tmp/lagom-manual.sqlite3 add-account test@lagom.test \
  --imap 127.0.0.1:3143 --smtp 127.0.0.1:3025 \
  --imap-security plain --smtp-security plain
# Enter the disposable test password at the hidden prompt; then use the same --database.
```

Provider settings checked against [Google](https://developers.google.com/workspace/gmail/imap/imap-smtp),
[Apple](https://support.apple.com/en-ie/102525),
[Fastmail](https://www.fastmail.help/hc/en-us/articles/1500000278342-Server-names-and-ports)
and [Microsoft](https://support.microsoft.com/en-us/outlook/pop-imap-and-smtp-settings-for-outlook-com).
Google and Microsoft support browser sign-in through `oauth::sign_in`.
Outlook password login is not expected to work. App passwords/provider IMAP
access must be enabled where supported for password accounts.

Dependencies: rusqlite/bundled supplies portable SQLite/FTS5; dirs locates app
data; keyring uses native credential stores; zeroize clears owned secret buffers;
uuid gives account ids; thiserror gives core errors. async-imap, Tokio,
futures-util and tokio-rustls provide the IMAP transport/streams; webpki-roots
supplies trust anchors; log disables unsafe wire traces; mail-parser handles
MIME/charsets/HTML text; lettre supplies SMTP and RFC message construction.
The CLI adds anyhow, clap and rpassword for errors, arguments and hidden input.
Tempfile is test-only. No OpenSSL or native-tls is enabled by the core.

## Workspace validation on 2026-11-06

`cargo build --workspace` and `LAGOM_GREENMAIL=1 cargo test --workspace` pass.
Core/CLI Clippy including all test targets passes with `-D warnings`.
Plain `cargo clippy --workspace` fails on an existing desktop
`clippy::approx_constant` at `apps/desktop/src/main_window.rs:120` (golden ratio).
The desktop is untouched. `cargo clippy --workspace -- -A clippy::approx_constant`
passes. Existing dependency future-compatibility notices remain for `block`
and `proc-macro-error2`.

## Sign-in providers

Lagom is a public installed client: browser authorization uses PKCE S256 and a
five-minute loopback listener. No Lagom server is involved. Refresh tokens use
the OS keychain, under the account UUID; access tokens are cached only in memory.
Dropping `oauth::sign_in` cancels it. Never log the authorization URL or tokens.

For Google, create a Google Cloud project, configure the OAuth consent screen,
and create an OAuth client of type **Desktop app**. Request the restricted Gmail
scope `https://mail.google.com/` plus `openid email profile`. In **Testing** mode,
add each tester under test users; Gmail refresh tokens in testing can expire
after seven days. Production distribution needs Google's verification for the
restricted scope (and any applicable security assessment). The desktop client
secret is non-confidential; Lagom sends it when configured. See
[Google's installed-app guide](https://developers.google.com/identity/protocols/oauth2/native-app).

For Microsoft, register an app in Entra with **Accounts in any organizational
directory and personal Microsoft accounts**. Add the **Mobile and desktop
applications** platform, redirect URI `http://localhost`, and enable public
client flows. Under **Office 365 Exchange Online**, add delegated permissions
`IMAP.AccessAsUser.All` and `SMTP.Send`; request `offline_access openid email
profile` too. Do not create a client secret. The `common` endpoint supports both
account types; organization policy may require admin consent or enabling IMAP
and authenticated SMTP. See [Microsoft's mail OAuth guide](https://learn.microsoft.com/en-us/exchange/client-developer/legacy-protocols/how-to-authenticate-an-imap-pop-smtp-application-by-using-oauth)
and [loopback redirect rules](https://learn.microsoft.com/en-us/entra/identity-platform/reply-url).

For a release, put your own registrations in the git-ignored `.env.oauth` at the
repository root (shell assignments; omit providers you do not configure):

```sh
LAGOM_GOOGLE_CLIENT_ID='<desktop-client-id>'
LAGOM_GOOGLE_CLIENT_SECRET='<desktop-client-secret>'
LAGOM_MICROSOFT_CLIENT_ID='<application-client-id>'
```

Run `scripts/build-release.sh`, or `scripts/build-release.sh path/to/local.env`
from the repository root. The script sources this trusted local shell file,
exports its assignments without printing values, and runs
`cargo build --release -p lagom`. Cargo embeds the values through `option_env!`;
the release binary does not need the file at runtime. Never commit the env file.

The same environment variables override compiled values at runtime. An empty
runtime ID disables that provider; `oauth::available(provider)` lets the UI hide
its button. The account's email/name come from the token endpoint's ID token;
Microsoft falls back to `preferred_username` when `email` is absent.

`discover(&str) -> Discovery` first checks known domains, then MX, then Mozilla
ISPDB. DNS and ISPDB each have a three-second deadline and fail through to the
next step. `Discovery::Password { preset, help_url }` includes both server
configurations; iCloud and Fastmail also include an app-password help link.
ISPDB accepts only TLS/required STARTTLS and supported password authentication;
otherwise discovery returns `Manual`. MX uses Hickory with system DNS settings;
quick-xml reuses the workspace XML parser for ISPDB. The [ISPDB format](https://github.com/thunderbird/autoconfig/wiki/Autoconfig-file-reference)
defines the supported email placeholders.

Desktop worker contract (`apps/desktop/src/sync.rs`):

- `Cmd::SignIn(Provider)` replaces a pending sign-in; `Cmd::CancelSignIn` drops it.
- `Cmd::Reauthenticate(String)` uses the stored account's provider and UUID.
  `oauth::reauthenticate` rejects a different email before writing credentials.
  `Event::Reauthenticated` returns success or a sanitized error for that account;
  cancellation leaves it requiring sign-in. Success restarts IDLE, queues sync,
  and resumes only outbox entries stopped by `ReauthRequired`.
  The sidebar and account rows offer Sign in again and cancellation while pending.
- `Event::OpenUrl(String)` must go to `cx.open_url` on the UI thread (wired in
  `main_window/live.rs`). Do not log or persist this URL.
- `Event::SignedIn(Result<String, String>)` returns the saved account UUID or a
  sanitized error; `Event::SignInCancelled` also covers provider access denial.
  Success emits `Changed`, starts IDLE, and queues a normal sync.
- `Cmd::Discover(String)` replaces pending discovery;
  `Event::Discovered { email, discovery }` identifies which input produced it.
- `Event::ReauthRequired(String)` identifies an account whose refresh grant was
  revoked. Core callers receive the distinct `Error::ReauthRequired`.

The worker owns these futures, so pending sign-in/discovery does not hold up
other commands. Network work stays off the UI thread. Onboarding buttons and
presentation of discovery belong to the subsequent UI change.
`ImapSync::login` and `smtp::{send, verify}` retain their password argument for
existing callers; OAuth accounts ignore that argument and use the memory cache
and keychain. Only an explicit authentication rejection causes one refresh and
retry. SMTP does not retry within an ambiguous submission (`MaybeSent`); the outbox
retries it on the next sync with the same Message-ID, which can produce a duplicate.

## OAuth validation on 2026-11-07

`cargo build --workspace` and `cargo test --workspace` pass: 99 tests pass,
with five GreenMail tests and the search benchmark ignored by default. The
worker's opt-in GreenMail test also skips its network work without its flag.
OAuth tests run a local fake authorization/token server and real loopback
callback for both providers, including PKCE/state, JWT identity, refresh-token
rotation, expiry, concurrent stale-token rejection, invalid_grant, and dropping
the sign-in future. Scripted IMAP/SMTP peers check XOAUTH2 bytes; discovery
uses fake MX/ISPDB responses and exercises fallbacks and timeouts. Migration
fixtures cover old password rows and both OAuth providers. CLI process tests
cover discovery and missing client IDs; worker tests cover command cancellation.

Core/CLI Clippy passes with `--all-targets -- -D warnings`. Workspace Clippy
passes with `--all-targets -- -D warnings -A clippy::approx_constant`; without
the exception it fails on the existing `main_window.rs:67` golden-ratio value.
No OpenSSL/native-tls occurs in the core dependency tree. Existing dependency
future-compatibility notices remain for `block` and `proc-macro-error2`.
Real provider consent, native keychain, Windows/Linux browser launch and release
performance were not verified; real client IDs are not available in this task.
