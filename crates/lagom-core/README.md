# Lagom mail core

Local-first Rust API; the desktop app is deliberately not wired to it yet.
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
attachment detection without fetching attachment bodies, and IDLE notification.

## Failure and sync semantics

Drop/reconnect an IMAP session after any network/protocol error or cancellation;
partially consumed commands must not be reused. No automatic retry or polling.
A failed operation may have reached the server; resync before retrying a move.
`Error::SentCopy` means SMTP succeeded: do not submit the message again.

Initial sync stores the newest 500 UIDs per folder by default; subsequent sync
keeps cached older messages and fetches every UID beyond the saved checkpoint.
UIDVALIDITY changes invalidate that folder's cached rows. Successful sync compares
all server UIDs to detect expunges. CONDSTORE refreshes changed flags across cached
UIDs; other servers refresh flags for the newest N. Snooze uses caller-supplied
Unix timestamps, with no background timer in the core.

// shortcut: cache the first preferred plain/HTML body part, combine multiple inline text parts when needed.
// shortcut: selected encoded text plus its headers must fit 4 MiB (headers <=64 KiB), add streaming bodies for larger mail.
// shortcut: deleted server folders remain locally listed, reconcile folder deletion when account-management UI lands.

TLS/STARTTLS against real providers and the real OS keychain require manual
account testing; GreenMail's integration ports exercise local plaintext only.
