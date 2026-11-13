# Release process

One git tag releases every platform. This page lists the version rule, the steps per platform
and the steps that need a human (certificates, store accounts, secrets).

## Version: the tag is the only source

- Release tags look like `v1.2.3`. A pre-release looks like `v1.2.3-beta.4`.
- `LAGOM_VERSION` is the tag without the `v`: `1.2.3` or `1.2.3-beta.4`.
- CI workflows set `LAGOM_VERSION` from `github.ref_name`. Local scripts take it as their first argument.
- `Cargo.toml` versions stay `0.0.0`. Do not bump them. Rust code that needs the app version reads
  `option_env!("LAGOM_VERSION")` and treats `None` as a development build.
- Numeric fields that refuse a suffix use the `1.2.3` part only.
- Release files are named `lagom-<LAGOM_VERSION>` plus a platform suffix and extension.

| Platform | Where the version goes                                                                       | Build number                                                            |
| -------- | -------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------- |
| macOS    | `CFBundleShortVersionString` = `1.2.3`, dmg `lagom-1.2.3.dmg`                                | `CFBundleVersion` = commit time, UTC `YYYYMMDDHHMM`                     |
| Windows  | `lagom.rc` `FILEVERSION`/`PRODUCTVERSION` = `1,2,3,0`, installer `lagom-1.2.3-windows-x64.*` | none                                                                    |
| Linux    | package version `1.2.3`, files `lagom-1.2.3-linux-x86_64.*`                                  | none                                                                    |
| iOS      | `MARKETING_VERSION` = `1.2.3` (pass it to `xcodebuild`, do not edit `project.yml`)           | `CURRENT_PROJECT_VERSION` = UTC minute (`release.sh` already does this) |
| Android  | `versionName` = `LAGOM_VERSION`                                                              | `versionCode` = major×1 000 000 + minor×10 000 + patch×100 + n          |

Android `n` is the pre-release number (`beta.4` gives 4) and 99 for the final release. So
`v1.2.3-beta.4` gives 1020304 and `v1.2.3` gives 1020399, and the final release always sorts last.

The Windows, Linux, iOS and Android rows are the agreed rule. Their scripts implement it in their own
branches. Not verified on `main` when this page was written.

## Cut a release

1. Merge everything for the release to `main`. CI must be green on `main`.
2. Do the manual checks in [Before you tag](#before-you-tag).
3. Tag and push: `git tag -s v1.2.3 -m "lagom 1.2.3" && git push origin v1.2.3`.
4. Each desktop packaging workflow builds on the tag and uploads its files to one **draft** GitHub
   release. The first workflow to finish creates the draft.
5. Download every file from the draft. Install and open each one on a clean machine or account.
6. Write the release notes and publish the draft. Publishing makes it the "latest" release.
7. Upload the iOS and Android builds to the stores (see below). Store review takes one to several days.

### Before you tag

- Sign in with Google, Microsoft and one plain IMAP account on a release build.
- Send, reply, archive and search on each account.
- Check that a notification appears for new mail.
- Read `TODO.md` for open items that block the release.

## macOS

Workflow: `.github/workflows/package-macos.yml`. Script: `scripts/package-macos.sh`.

- The dmg holds one universal app (Apple Silicon and Intel), built with `lipo`. Minimum macOS 13.
- Tags `v*`: build, sign with the Developer ID, notarize, staple and upload to the draft release.
- Pull requests that change packaging: build an ad-hoc signed dmg as a workflow artifact. Pull
  requests never get the signing secrets.
- `workflow_dispatch` on a branch: as a tag, but the version is `0.0.0-dev.<sha>` and nothing goes to
  a release.
- Without the signing secrets the workflow still builds an ad-hoc signed dmg and warns. Gatekeeper
  blocks that dmg on other Macs. Use it only for tests.
- No entitlements file. The app is not sandboxed, and the hardened runtime allows the network,
  keychain, notifications and WKWebView.

Local release from a Mac with the Developer ID in the login keychain:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin   # once
scripts/package-macos.sh 1.2.3                                # writes target/macos/lagom-1.2.3.dmg
```

`LAGOM_SIGN_IDENTITY=-` signs ad hoc and skips notarization. `LAGOM_NOTARIZE=0` signs but skips
notarization. Notarization uses the keychain profile `lagom-notary` unless `LAGOM_NOTARY_KEY`,
`LAGOM_NOTARY_KEY_ID` and `LAGOM_NOTARY_ISSUER` are set.

### GitHub secrets for macOS

| Secret                       | Content                                                                                                                                            |
| ---------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| `MACOS_CERTIFICATE_P12`      | Base64 of the "Developer ID Application" certificate and private key, exported as .p12                                                             |
| `MACOS_CERTIFICATE_PASSWORD` | The .p12 export password                                                                                                                           |
| `APPLE_NOTARY_KEY`           | The full text of the App Store Connect API key file `AuthKey_<id>.p8`                                                                              |
| `APPLE_NOTARY_KEY_ID`        | The API key ID (10 characters)                                                                                                                     |
| `APPLE_NOTARY_ISSUER_ID`     | The issuer ID (a UUID) from the same App Store Connect page                                                                                        |
| `LAGOM_OAUTH_ENV`            | The full text of `.env.oauth` (Google and Microsoft client IDs). Optional; without it OAuth sign-in is off. Windows and Linux use the same secret. |

### Human steps for macOS (once)

1. In the Apple Developer account (team `3KFU9JQ5LH`), create a **Developer ID Application**
   certificate. This needs the Account Holder role or an Admin with Developer ID access.
2. In Keychain Access, export the certificate with its private key as `lagom-devid.p12` with a strong password.
3. In App Store Connect, go to Users and Access, Integrations, Team Keys. Create a key with the
   **Developer** role. Download the .p8 file (Apple allows one download). Note the key ID and issuer ID.
4. Store the secrets:

   ```sh
   base64 -i lagom-devid.p12 | gh secret set MACOS_CERTIFICATE_P12
   gh secret set MACOS_CERTIFICATE_PASSWORD
   gh secret set APPLE_NOTARY_KEY < AuthKey_XXXXXXXXXX.p8
   gh secret set APPLE_NOTARY_KEY_ID --body XXXXXXXXXX
   gh secret set APPLE_NOTARY_ISSUER_ID --body xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
   gh secret set LAGOM_OAUTH_ENV < .env.oauth
   ```

5. Delete the local .p12 and .p8 copies, or keep them only in a password manager.
6. Run the workflow once with `workflow_dispatch` and check that the log shows `Authority=Developer ID Application`
   and an accepted notarization.

## Windows

- Build the installer on `windows-latest` from the tag. Set `FILEVERSION`/`PRODUCTVERSION` in
  `apps/desktop/lagom.rc` from `LAGOM_VERSION` at build time.
- Upload to the same draft release with `gh release upload --clobber`. Create the draft when it is missing and stop when the release is already published,
  as `package-macos.yml` does.
- Human step: get a code signing service. Azure Trusted Signing (Artifact Signing) is the cheapest
  route for an individual. An OV or EV certificate on a hardware token also works. Unsigned installers
  get a SmartScreen warning. Signed ones still warn until the publisher earns reputation.
- Store the signing credentials as GitHub secrets. Document their names next to the workflow.

## Linux

Script: `scripts/package-linux.sh [version]`. Workflow: `.github/workflows/package-linux.yml`.

- Builds `lagom-<LAGOM_VERSION>-linux-x86_64.deb` and `.AppImage` on `ubuntu-22.04`, not
  `ubuntu-latest`: the packages need at least the glibc they were built with, so the oldest
  supported Ubuntu reaches the most distributions.
- The .deb version replaces `-` with `~` (`1.2.3~beta.4`), so a pre-release sorts before `1.2.3`.
- CI installs the .deb, starts lagom through D-Bus activation and the AppImage under Xvfb.
- Uploads to the same draft release, as for macOS.
- No signing certificate is needed. Optional: publish a `SHA256SUMS` file with the release.
- Human step: set `LAGOM_DEB_MAINTAINER` (or the script default) to a real contact address.
- Human step for a Flathub or Snap listing: create the store account and submit the manifest.

## iOS and iPadOS

Script: `apps/mobile/apple/release.sh [--upload]`. It builds the Rust library, archives with
Xcode automatic signing (team `3KFU9JQ5LH`) and exports or uploads the build.

1. Pass the version: the script must call `xcodebuild ... MARKETING_VERSION="${LAGOM_VERSION%%-*}"`.
   Not done on `main` when this page was written.
2. Run `apps/mobile/apple/release.sh --upload` on a Mac that is signed in to Xcode with the team.
3. In TestFlight, test the build on an iPhone and an iPad.
4. Submit for review in App Store Connect.

Human steps (once per app, then per release where noted):

- Create the app record for `com.alexkerber.lagom` in App Store Connect.
- Fill in the privacy nutrition labels. lagom collects no data; mail stays between the device and the
  mail server.
- Add screenshots in the sizes App Store Connect asks for (iPhone and iPad), a description, keywords, a support URL and a
  privacy policy URL (per release when the UI changes).
- Give App Review a test IMAP account and password in the review notes (per release).
- Export compliance: `ITSAppUsesNonExemptEncryption` is already `false` in `project.yml`.

## Android

1. Build a signed App Bundle with `./gradlew bundleRelease` after `scripts/build-android.sh`.
   `build.gradle.kts` has no release signing config and no version from `LAGOM_VERSION` yet. The
   Android branch adds both: read the keystore path and passwords from environment variables or
   GitHub secrets, never from the repository.
2. Upload the `.aab` to the Play Console, internal testing track first, then production.

Human steps (once):

- Create the Play Console developer account and the app `com.alexkerber.lagom`.
- Create an upload keystore (`keytool -genkeypair -v -keystore lagom-upload.jks -keyalg RSA -keysize 4096 -validity 10000 -alias upload`).
  Keep it and its passwords in a password manager. Turn on Play App Signing.
- New personal developer accounts must run a closed test with at least 12 testers for 14 days
  before production access. Plan for this.
- Fill in the Data safety form, content rating, target audience and store listing (icon
  `brand/app-icon/export/play-store-512.png`, feature graphic, screenshots).

## OAuth providers (all platforms)

- Google: the scope `https://mail.google.com/` is a restricted scope. A public release needs Google's
  OAuth app verification and a yearly CASA security assessment. Until then, only test users added in
  the Google Cloud console can sign in (100 at most). Start this early; it can take weeks.
- Microsoft: complete publisher verification in Microsoft Entra so users do not see "unverified".

## In-app update check: decision

Decision: a small "new version available" check against GitHub releases fits. Sparkle does not fit now.
Not implemented on this branch. Tracked as an open item below.

- Sparkle installs updates itself, but it is macOS only. It needs an embedded framework, an EdDSA key,
  a hosted appcast and Objective-C glue from Rust. Revisit when automatic installs matter.
- The check that fits:
  - At most once a day, send `GET https://api.github.com/repos/alexkerber/lagom-app/releases/latest` with
    the reqwest client in `lagom-core`. Send no cookies and no identifiers. GitHub sees the IP address
    and a fixed user agent only.
  - Compare `tag_name` with `LAGOM_VERSION` as numbers. Skip the check in development builds.
  - When the release is newer, show one quiet row: "lagom 1.2.4 is available" with a link to the
    release page. No download in the app.
  - Settings get a "Check for updates" switch, on by default, saved with the other settings.
  - The same code serves macOS, Windows and Linux. The stores update iOS and Android.
- Why it is not on this branch: it needs a settings switch and a row in the main window. Other
  branches change those files now. Do it after they merge.

## Open items

- [ ] Desktop: the update check described above.
- [ ] iOS: pass `MARKETING_VERSION` from `LAGOM_VERSION` in `release.sh`.
- [ ] Android: release signing config and `versionName`/`versionCode` from `LAGOM_VERSION`.
- [ ] Windows: a packaging workflow that follows this page. Linux: `package-linux.yml` (branch `release/linux`).
