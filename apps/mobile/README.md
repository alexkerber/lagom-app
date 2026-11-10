# Lagom mobile

Native apps on `lagom-core`, with no mail business logic in Swift.

| Path | Platforms | UI |
| --- | --- | --- |
| `apps/mobile/apple` | iPhone and iPad, iOS 18+ | SwiftUI |
| `apps/mobile/android` | Planned | Jetpack Compose |

The first Apple slice uses `crates/lagom-ffi`, with [UniFFI 0.32.2](https://docs.rs/uniffi/0.32.2/uniffi/)
pinned and proc-macro-generated Swift bindings. One serialized Rust worker owns
IMAP actions, discovery and credentials. A separate SQLite WAL connection keeps
cached reads and durable enqueue available during network waits.
Swift owns navigation, form state and presentation. Mailbox reads include all
accounts, order by date then ID descending, and page in batches of 100. Refreshes
retain the loaded range until the account or mailbox changes.

## Build

Use an Apple Silicon Mac with Xcode, Rust targets `aarch64-apple-ios` and
`aarch64-apple-ios-sim`, and XcodeGen. No running simulator is needed.
From the repository root:

```sh
scripts/build-apple.sh
xcodegen generate --spec apps/mobile/apple/project.yml
xcodebuild build \
  -project apps/mobile/apple/Lagom.xcodeproj -scheme Lagom \
  -destination 'generic/platform=iOS Simulator' \
  -derivedDataPath apps/mobile/apple/DerivedData CODE_SIGNING_ALLOWED=NO
```

The script builds both release static libraries, runs the workspace's
`uniffi-bindgen` binary and packages `Generated/LagomFFI.xcframework` and
`Generated/LagomCore.swift`. Generated files, the `.xcodeproj` and DerivedData
are ignored; regenerate them after FFI changes. The generated project targets
arm64 on devices and simulators. Intel simulator builds are not included.
CI adds an independent macOS Apple build job; existing workspace jobs remain.

`keyring`'s `apple-native` iOS implementation and `zed-reqwest` compile for both
Apple targets, so the desktop credential and HTTP backends are unchanged.
Keychain access uses the app's sandbox entitlements. Compilation does not verify
runtime credential access on signed hardware.

## Run manually

Generate the project as above, then use Xcode to select a simulator **you own**
or your development iPhone/iPad. For a physical device, select your development
team under Signing & Capabilities if you are not on the project's team
(`com.alexkerber.lagom`, team 3KFU9JQ5LH). Build and Run is a manual acceptance step;
the automated build commands neither launch the app nor install it anywhere.
Never use, boot, reset, erase or install on somebody else's simulator.

Add an account with email and password, or expand Advanced for separate IMAP
and SMTP hosts, ports, usernames and TLS/STARTTLS. Gmail/iCloud need an
app-specific password. Microsoft password authentication is commonly disabled;
Google/Microsoft OAuth needs mobile client IDs and is outside this slice.

The app has the desktop's four pages: Today, Mail, Calendar and Todo. Compact
layouts use a tab bar; regular width and height (iPad, unfolded phones) use the
desktop sidebar: the four pages, Mail's mailboxes with a per-account Inbox and
account colour dots, then Accounts. The last page is restored on launch; Mail on
first launch. In Mail, regular layouts show sidebar, list and message in three
columns; compact layouts open a message from its list. The list groups mail
under Today, Yesterday, this week's weekdays and Earlier, marks each account with
its colour when several accounts exist, filters All/Unread on loaded rows and
shows how much Inbox mail is left.

Today shows today's cached events, the tasks core selects for today, and the
newest unread Inbox mail. Todo lists, adds (with an optional due date),
completes and deletes local tasks through core's task store. Calendar is a
read-only agenda of the next 14 days of cached events. Mobile has no Google
sign-in yet, so password accounts show an empty state that says the calendar
needs a Google account. Done archives on the server; Delete moves to
server Trash. Later is local and hides mail until tomorrow at 08:00 in the
system time zone. A local foreground timer reloads at the next snooze deadline.
Pull to refresh, launch and foreground entry sync all accounts.

Send commits Markdown mail to the outbox before dismissing compose. Sync
attempts delivery and retries temporary failures on subsequent syncs, preserving
core's backoff and SMTP/Sent checkpoints. Outbox failures remain visible; Send
again requires confirmation because uncertain delivery may duplicate a message.
The message toolbar's reply menu offers Reply, Reply all and Forward. Rust shares
subject, quote and signature formatting with desktop. Reply all keeps Cc, excludes
the source account's email/login identities and deduplicates mailbox addresses.
Replies keep the original account, Message-ID and References in Rust; forwards
start a new thread with empty recipients. The composer exposes editable To and Cc.
Forwarding includes cached attachments and fetches missing parts through core before
opening compose. A fetch failure keeps the sheet closed and shows an error; older
messages without attachment metadata need a refresh first. Aliases beyond the
account email and logins are not yet stored. Attachment picking is not included.

## Demo data

DEBUG builds launched with `-LagomDemo` open a seeded fixture store in the app's
temporary directory (two sample accounts, mail, tasks and today's events) and
skip network sync. Real accounts and the Keychain are never touched. Add
`-lagom.section today` (or `mail`, `calendar`, `todo`) to open a page:

```sh
xcrun simctl launch <your-simulator-udid> com.alexkerber.lagom -LagomDemo -lagom.section today
```

HTML uses a nonpersistent WKWebView, JavaScript disabled, and the same CSP and
resource-hint blocking policy as desktop. Remote images remain blocked; a
plain-text toggle is available. Attachment metadata is displayed; opening
attachments and picking new files are outside this slice. Compose text is held in the sheet until Send;
draft autosave/restoration is not implemented. No Android, calendar, todos,
push or background fetch is included. Mobile cannot create events or edit task
titles and due dates after creation yet.

## Verify without opening an app

```sh
cargo build --workspace
cargo test --workspace
xcodebuild build-for-testing \
  -project apps/mobile/apple/Lagom.xcodeproj -scheme Lagom \
  -destination 'generic/platform=iOS Simulator' \
  -derivedDataPath apps/mobile/apple/DerivedData CODE_SIGNING_ALLOWED=NO
```

Rust FFI tests use temporary stores, an in-memory keyring backend and loopback
IMAP/SMTP fixtures. They cover global paging/ties, account isolation, Later,
message/attachment mapping, read-status refusal, archive/Trash, verified account
creation, durable replies, recovery and shared HTML privacy policy. Existing
GreenMail tests remain opt-in. `LagomTests` is a hostless XCTest bundle: it compiles
the model, presentation and bindings directly, so it never launches the mail app or opens its store. Its
network-free tests cover mapping plus reply/forward preparation, stale selection,
duplicate actions, preserved edits, Cc, attachments and enqueue failures using a
fake core, plus list dates, date groups, due chips, stored page names and the
Today/Todo model (blank titles, local due days, completion) against a fake core. `build-for-testing` only compiles tests. Execute them with
`xcodebuild test-without-building` on a disposable simulator you create, targeting
only its UDID, then delete that simulator. Never target someone else's device.
Rust fixtures additionally cover reply-all identities, quoted originals, signatures,
thread headers, durable attachment enqueue, cache reuse, loopback fetches and
attachment failures.

The generated Swift async boundary can also be checked on the host without
XCTest, an app or a simulator (run the Apple generation and workspace build first):

```sh
xcrun swiftc apps/mobile/apple/Generated/LagomCore.swift \
  crates/lagom-ffi/tests/swift-smoke.swift \
  -I apps/mobile/apple/Generated/LagomFFI.xcframework/ios-arm64/Headers \
  -L target/debug -llagom_ffi -framework Security -framework SystemConfiguration \
  -lresolv -liconv -o target/mobile-swift-smoke
target/mobile-swift-smoke
xcrun swiftc apps/mobile/apple/Generated/LagomCore.swift \
  apps/mobile/apple/Sources/MailModel.swift crates/lagom-ffi/tests/mobile-model.swift \
  -I apps/mobile/apple/Generated/LagomFFI.xcframework/ios-arm64/Headers \
  -L target/debug -llagom_ffi -framework Security -framework SystemConfiguration \
  -lresolv -liconv -o target/mobile-model-tests
target/mobile-model-tests
target/mobile-model-tests --wake
```

This checks seven fixture-only FFI contracts, including the Swift/Tokio async
bridge and readable errors, plus two MailModel regressions for retained pages
and foreground snooze deadlines using a fake core. Rust fixtures also verify
local access during a stalled sync, Done on snoozed archived mail and the earliest
snooze across accounts. These checks never use real accounts or keychains. Native
layout, navigation, gestures, WKWebView behavior, Dynamic Type and signed-device
Keychain/provider acceptance still require manual testing.

## Remaining daily-mail work

This slice stops after Reply, Reply all and Forward. Message-scoped CID rendering
and opt-in remote images, draft autosave/restoration and discard, unified search,
and explicit read/unread actions remain queued in that order. Sending still saves
to the outbox durably; closing an unsent composer still loses its text.

## TestFlight

`apps/mobile/apple/release.sh` builds the bindings, generates the project and
archives a Release build for devices with automatic signing, using the Apple ID
signed in to Xcode. The build number is the UTC time (`YYYYMMDDHHMM`), so every
archive is newer than the last. Without arguments it exports an IPA to
`apps/mobile/apple/build/export`; with `--upload` the export goes straight to
App Store Connect through the same Xcode account (no API key). The app record
for `com.alexkerber.lagom` must exist in App Store Connect first.
