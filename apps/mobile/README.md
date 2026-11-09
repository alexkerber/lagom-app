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

The compact layout opens a message from its list. Regular width and height
show accounts/mailboxes, list and message in three columns, including phones
that report those size classes. Done archives on the server; Delete moves to
server Trash. Later is local and hides mail until tomorrow at 08:00 in the
system time zone. A local foreground timer reloads at the next snooze deadline.
Pull to refresh, launch and foreground entry sync all accounts.

Send commits Markdown mail to the outbox before dismissing compose. Sync
attempts delivery and retries temporary failures on subsequent syncs, preserving
core's backoff and SMTP/Sent checkpoints. Outbox failures remain visible; Send
again requires confirmation because uncertain delivery may duplicate a message.
Replies keep the original account, Message-ID and References in Rust.

HTML uses a nonpersistent WKWebView, JavaScript disabled, and the same CSP and
resource-hint blocking policy as desktop. Remote images remain blocked; a
plain-text toggle is available. Attachment metadata is displayed; download and
upload are outside this slice. Compose text is held in the sheet until Send;
draft autosave/restoration is not implemented. No Android, calendar, todos,
push or background fetch is included.

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
GreenMail tests remain opt-in. `LagomTests` contains four network-free XCTest
mapping tests; `build-for-testing` compiles them without executing XCTest.

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

## TestFlight

`apps/mobile/apple/release.sh` builds the bindings, generates the project and
archives a Release build for devices with automatic signing, using the Apple ID
signed in to Xcode. The build number is the UTC time (`YYYYMMDDHHMM`), so every
archive is newer than the last. Without arguments it exports an IPA to
`apps/mobile/apple/build/export`; with `--upload` the export goes straight to
App Store Connect through the same Xcode account (no API key). The app record
for `com.alexkerber.lagom` must exist in App Store Connect first.
