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

| Platform | Where the version goes                                                                       | Build number                                                                           |
| -------- | -------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| macOS    | `CFBundleShortVersionString` = `1.2.3`, dmg `lagom-1.2.3.dmg`                                | `CFBundleVersion`: dmg = the Android rule; App Store = commit time, UTC `YYYYMMDDHHMM` |
| Windows  | `lagom.rc` `FILEVERSION`/`PRODUCTVERSION` = `1,2,3,0`, installer `lagom-1.2.3-windows-x64.*` | none                                                                                   |
| Linux    | package version `1.2.3`, files `lagom-1.2.3-linux-x86_64.*`                                  | none                                                                                   |
| iOS      | `MARKETING_VERSION` = `1.2.3` (pass it to `xcodebuild`, do not edit `project.yml`)           | `CURRENT_PROJECT_VERSION` = UTC minute (`release.sh` already does this)                |
| Android  | `versionName` = `LAGOM_VERSION`                                                              | `versionCode` = major×1 000 000 + minor×10 000 + patch×100 + n                         |

Android `n` is the pre-release number (`beta.4` gives 4) and 99 for the final release. So
`v1.2.3-beta.4` gives 1020304 and `v1.2.3` gives 1020399, and the final release always sorts last.

The dmg uses the same number as `CFBundleVersion`, because Sparkle offers an update only when that
number is higher. A beta and the final release on one commit still differ. Development builds
(`0.0.0-dev.<sha>`) get 0. The App Store build keeps the commit time: App Store Connect already has
builds with that number, and a store build number must not go down.

The Windows, Linux and Android rows are the agreed rule. Their scripts implement it in their own
branches. Not verified on `main` when this page was written.

## Cut a release

1. Merge everything for the release to `main`. CI must be green on `main`.
2. Do the manual checks in [Before you tag](#before-you-tag).
3. Tag and push: `git tag -s v1.2.3 -m "lagom 1.2.3" && git push origin v1.2.3`.
4. Each desktop packaging workflow builds on the tag and uploads its files to one **draft** GitHub
   release. The first workflow to finish creates the draft.
5. Download every file from the draft. Install and open each one on a clean machine or account.
6. Check that the tag has one draft with every platform's files. Only `release-draft.yml` creates it, at tag push; the platform uploads wait for it (`scripts/upload-release.sh`). Check that `lagom-<version>.appcast.xml` is there next to the dmg. Write the release notes and publish the draft. Publishing makes it the "latest" release and puts the dmg in the update feed (see [Automatic updates on macOS](#automatic-updates-on-macos)).
7. Upload the iOS and Android builds to the stores (see below). Store review takes one to several days.

### Before you tag

- Sign in with Google, Microsoft and one plain IMAP account on a release build.
- Send, reply, archive and search on each account.
- Check that a notification appears for new mail.
- Read `TODO.md` for open items that block the release.

## Release credentials: the `release` environment

Signing and upload secrets live in the GitHub environment `release`, not in the repository secrets.

- The environment's deployment policy admits only tags that match `v*`. A job with
  `environment: release` on a branch or a pull request stops at the gate and gets no secret.
- A tag ruleset lets only repository admins create, update or delete `v*` tags. So only an admin
  starts a run that reaches the secrets.
- A job that holds a release credential runs no cargo, no rustup, no build script and no code from
  an artifact. It runs on a fresh GitHub-hosted runner, checks out `github.sha` and checks each
  artifact against a hash from the job that made it. A compromised Rust dependency then runs code
  only in jobs without a credential. That job still signs what the build made: the split keeps the
  keys away from the build, it does not prove the build benign.
- Environment secrets: `MACOS_CERTIFICATE_P12`, `MACOS_CERTIFICATE_PASSWORD`, `APPLE_NOTARY_KEY`,
  `APPLE_NOTARY_KEY_ID`, `APPLE_NOTARY_ISSUER_ID` and `SPARKLE_ED_PRIVATE_KEY` (macOS).
  `APP_STORE_CONNECT_KEY`, `APP_STORE_CONNECT_KEY_ID`, `APP_STORE_CONNECT_ISSUER_ID` and the optional
  `IOS_SIGNING_P12` and `IOS_SIGNING_P12_PASSWORD` (iOS). `LAGOM_ANDROID_KEYSTORE`,
  `LAGOM_ANDROID_KEYSTORE_PASSWORD`, `LAGOM_ANDROID_KEY_ALIAS` and the optional
  `LAGOM_ANDROID_KEY_PASSWORD` (Android).
- Repository secret: only `LAGOM_OAUTH_ENV` (client IDs that ship in the app, a build input).
- Store a secret with `gh secret set <NAME> --env release`. After you move one, delete the
  repository copy with `gh secret delete <NAME>`: a workflow on any branch can read a repository secret.
  Move a platform's secrets when its workflow split is on `main`; the old workflow reads repository secrets.
- A `workflow_dispatch` on a branch never asks for the environment. It builds without signing.
- Not verified: a tag run with the environment (written 2026-11-21).

## macOS

Workflow: `.github/workflows/package-macos.yml`. Scripts: `scripts/package-macos.sh` builds the
app, `scripts/sign-macos.sh` signs it and makes the dmg.

- The dmg holds one universal app (Apple Silicon and Intel), built with `lipo`. Minimum macOS 13.
- Tags `v*`: build, sign with the Developer ID, notarize, staple and upload to the draft release.
- Pull requests that change packaging: build an ad-hoc signed dmg as a workflow artifact. Pull
  requests never get the signing secrets.
- `workflow_dispatch` on a branch, `main` included: only the ad-hoc dmg. The signing secrets are in
  the `release` environment, which admits only `v*` tags. So a branch cannot get a notarized dmg
  with its own Sparkle key file. `workflow_dispatch` on a `v*` tag runs as a tag push.
- The ad-hoc dmg is for tests only: Gatekeeper blocks it on other Macs. When the environment lacks a
  signing secret, a tag run fails in `sign` and attaches nothing to the release.
- No entitlements file. The app is not sandboxed, and the hardened runtime allows the network,
  keychain, notifications and WKWebView.

CI jobs. No job that runs cargo, a build script or code from an artifact gets a release credential:

| Job       | Runs                                                                                                                                                         | Secrets                                             |
| --------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------- |
| `meta`    | Sets the version. Signs only for `v*` tags.                                                                                                                  | None                                                |
| `build`   | cargo, `LAGOM_BUILD_ONLY=1 scripts/package-macos.sh`. Uploads the unsigned app as a tar, and an ad-hoc dmg.                                                  | `LAGOM_OAUTH_ENV` (client IDs that ship in the app) |
| `sign`    | `environment: release`. A fresh runner, a sparse checkout of this commit, `scripts/sign-macos.sh`. No cargo, nothing from the artifact runs. `v*` tags only. | `MACOS_CERTIFICATE_*`, `APPLE_NOTARY_*`             |
| `sparkle` | `environment: release`. A fresh runner. Checks the dmg hash from `sign`, then `scripts/sparkle-item.sh`. `v*` tags only.                                     | `SPARKLE_ED_PRIVATE_KEY`                            |
| `release` | Signed tags only. No environment. Downloads the named artifacts, checks their hashes, attaches them to the draft.                                            | `GITHUB_TOKEN` with `contents: write`               |

Threat model. A compromised dependency can run code in `build`, through its build script. Before
this split it could use the unlocked Developer ID and read the notary key. With Sparkle, a Developer
ID-signed dmg with a new EdDSA key can pass Sparkle's key-change path, so that identity could
authorize updates. Now `build` has no credential. `sign` checks that the app is the dmg build of
this version with this commit's Sparkle key. It cannot prove that the code in the app is benign:
the isolation keeps the keys away from the build, but `sign` still signs what `build` made. The
rust cache that `build` saves feeds only later `build` jobs, which hold no credential. Not run with
the real secrets yet (2026-11-20); the first tag run is the test.

A branch can edit this workflow, but it cannot read the signing or Sparkle secrets: they are in the
`release` environment (see [Release credentials](#release-credentials-the-release-environment)).
Only an admin can push a `v*` tag.

Local release from a Mac with the Developer ID in the login keychain. One command builds and signs:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin   # once
scripts/package-macos.sh 1.2.3                                # writes target/macos/lagom-1.2.3.dmg
```

The same in two steps, as CI does: `LAGOM_BUILD_ONLY=1 scripts/package-macos.sh 1.2.3`, then
`scripts/sign-macos.sh 1.2.3`.

`LAGOM_SIGN_IDENTITY=-` signs ad hoc and skips notarization. `LAGOM_NOTARIZE=0` signs but skips
notarization. Notarization uses the keychain profile `lagom-notary` unless `LAGOM_NOTARY_KEY`,
`LAGOM_NOTARY_KEY_ID` and `LAGOM_NOTARY_ISSUER` are set.

### GitHub secrets for macOS

All except `LAGOM_OAUTH_ENV` are secrets of the `release` environment. `LAGOM_OAUTH_ENV` is a
repository secret, because the `build` job reads it.

| Secret                       | Content                                                                                                                                            |
| ---------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| `MACOS_CERTIFICATE_P12`      | Base64 of the "Developer ID Application" certificate and private key, exported as .p12                                                             |
| `MACOS_CERTIFICATE_PASSWORD` | The .p12 export password                                                                                                                           |
| `APPLE_NOTARY_KEY`           | The full text of the App Store Connect API key file `AuthKey_<id>.p8`                                                                              |
| `APPLE_NOTARY_KEY_ID`        | The API key ID (10 characters)                                                                                                                     |
| `APPLE_NOTARY_ISSUER_ID`     | The issuer ID (a UUID) from the same App Store Connect page                                                                                        |
| `LAGOM_OAUTH_ENV`            | The full text of `.env.oauth` (Google and Microsoft client IDs). Optional; without it OAuth sign-in is off. Windows and Linux use the same secret. |
| `SPARKLE_ED_PRIVATE_KEY`     | The Sparkle EdDSA private key. Only `scripts/sparkle-keys.sh` sets it. Without it a tag build gets no appcast item.                                |

### Human steps for macOS (once)

1. In the Apple Developer account (team `3KFU9JQ5LH`), create a **Developer ID Application**
   certificate. This needs the Account Holder role or an Admin with Developer ID access.
2. In Keychain Access, export the certificate with its private key as `lagom-devid.p12` with a strong password.
3. In App Store Connect, go to Users and Access, Integrations, Team Keys. Create a key with the
   **Developer** role. Download the .p8 file (Apple allows one download). Note the key ID and issuer ID.
4. Store the secrets:

   ```sh
   base64 -i lagom-devid.p12 | gh secret set MACOS_CERTIFICATE_P12 --env release
   gh secret set MACOS_CERTIFICATE_PASSWORD --env release
   gh secret set APPLE_NOTARY_KEY --env release < AuthKey_XXXXXXXXXX.p8
   gh secret set APPLE_NOTARY_KEY_ID --env release --body XXXXXXXXXX
   gh secret set APPLE_NOTARY_ISSUER_ID --env release --body xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
   gh secret set LAGOM_OAUTH_ENV < .env.oauth
   ```

5. Delete the local .p12 and .p8 copies, or keep them only in a password manager.
6. The first `v*` tag is the test: check that the `sign` log shows `Authority=Developer ID Application`
   and an accepted notarization. A dispatch on a branch does not sign.

### Automatic updates on macOS

The dmg updates itself with [Sparkle 2](https://sparkle-project.org) (2.10.1, pinned with its
checksum in `scripts/sparkle.sh`). The App Store build has no Sparkle: the store updates it.

- The app menu has "Check for Updates…" under About. Settings, General has "Check for updates
  automatically", on by default. Sparkle saves that switch in its own user defaults.
- Sparkle checks once a day, downloads in the background and installs when lagom quits. "Check for
  Updates…" shows Sparkle's own window with Install and Relaunch.
- Feed: `https://lagommail.app/appcast.xml`, on GitHub Pages. `pages.yml` builds it on every deploy
  with `scripts/appcast.sh`, from the `lagom-<version>.appcast.xml` files of the 20 newest
  **published** releases. Publishing a release redeploys the site from `main`. Drafts never appear,
  because their download links do not work.
- Every release is a beta today, so the feed has betas. A separate beta channel
  (`sparkle:channel`) comes when stable releases start.
- Security: every dmg is signed with an EdDSA key (`sign_update`), and the app has the public key
  as `SUPublicEDKey`. `SUVerifyUpdateBeforeExtraction` checks the signature before Sparkle opens the
  dmg. The feed is HTTPS only. Sparkle accepts an update when the EdDSA signature matches. Its only
  other path is for a key change: the new dmg and app carry the same Developer ID team. It always
  rejects an app with a broken code signature.
- `scripts/package-macos.sh` embeds `Sparkle.framework` in `Contents/Frameworks` without its XPC
  services (they serve sandboxed apps only). `scripts/sign-macos.sh` signs Autoupdate, Updater.app,
  the framework and then the app with the Developer ID and the hardened runtime.
- `scripts/sparkle-item.sh <version>` signs a notarized dmg and writes its `<item>`. The local release
  script runs it with the Keychain key. CI runs it only for tags, in its own job on a fresh runner
  that runs no cargo, so build scripts of dependencies never see the private key.
- Sparkle accepts a dmg signed by the same Developer ID team as a key change, so that identity also
  authorizes updates. CI keeps it in the `sign` job, away from cargo (see "CI jobs" above).
- Without `apps/desktop/macos/sparkle-ed-public-key` the dmg has no Sparkle, and CI warns.
- An ad-hoc signed dmg (pull requests, `LAGOM_SIGN_IDENTITY=-`) contains Sparkle, but the hardened
  runtime refuses to load it, so that build has no updater. Measured 2026-11-20; Developer ID
  signing loads it.
- Test on your Mac: a debug build reads `LAGOM_SPARKLE_FEED` as the feed URL (release builds do
  not), so it can point at an appcast on `127.0.0.1` signed with a throwaway key.

Key setup (once, on the Mac that signs releases):

```sh
scripts/sparkle-keys.sh   # needs gh signed in; Keychain asks to allow generate_keys
git add apps/desktop/macos/sparkle-ed-public-key && git commit -m "macOS: Sparkle public key"
```

The script makes the key pair in the login Keychain (account `lagom`), or keeps the one there. It
stores the private key as `SPARKLE_ED_PRIVATE_KEY` in the `release` environment
(`gh secret set --env release`) through a temp file that only you can read and
that it deletes. It prints and writes only the public key. Back up the private key once in a
password manager: `target/sparkle/bin/generate_keys --account lagom -x <file>`, then delete the file. Without the key,
no update reaches the installed apps.

Local release: `scripts/package-macos.sh 1.2.3` also writes `target/macos/lagom-1.2.3.appcast.xml`.
Upload it with the dmg: `scripts/upload-release.sh v1.2.3 target/macos/lagom-1.2.3.dmg target/macos/lagom-1.2.3.appcast.xml`.

### Mac App Store

`LAGOM_MAS=1 scripts/package-macos.sh 1.2.3` builds the store version. Add `LAGOM_MAS_UPLOAD=1` to upload it.

- The app is sandboxed with `apps/desktop/macos/appstore.entitlements`: network client, Downloads and
  files the user picks. Its data lives in `~/Library/Containers/com.alexkerber.lagom`, apart from the dmg
  version's data. The two versions do not share accounts.
- The bundle ID is the iOS one, `com.alexkerber.lagom`. Add macOS as a platform on the same App Store
  Connect record: one name, one universal purchase.
- The script signs ad hoc with the entitlements, wraps the app in an `.xcarchive` and lets
  `xcodebuild -exportArchive` sign it for the store with automatic signing (team `3KFU9JQ5LH`), as the
  iOS release does. No certificate to create by hand. Not verified end to end: App Store Connect
  answered 500 on 2026-11-17.
- No dmg and no notarization; the store does both jobs.

## Windows

- Build the installer on `windows-latest` from the tag. Set `FILEVERSION`/`PRODUCTVERSION` in
  `apps/desktop/lagom.rc` from `LAGOM_VERSION` at build time.
- Upload to the same draft release with `scripts/upload-release.sh` (it waits for the draft from `release-draft.yml`) and stop when the release is already published,
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

Workflow: `.github/workflows/package-ios.yml`. Script: `apps/mobile/apple/release.sh <version> [--upload | --unsigned]`.

- The script builds the Rust library, generates the Xcode project and archives the Release build. It passes
  `MARKETING_VERSION=${LAGOM_VERSION%%-*}` and `CURRENT_PROJECT_VERSION` = the UTC minute to `xcodebuild`.
  `project.yml` keeps development values. A pre-release `1.2.3-beta.4` ships as `1.2.3`, and TestFlight
  tells builds apart by the build number.
- Signing is automatic (team `3KFU9JQ5LH`): the Apple ID in Xcode, or an App Store Connect API key when
  `LAGOM_ASC_KEY` (path to the .p8), `LAGOM_ASC_KEY_ID` and `LAGOM_ASC_ISSUER` are set.
- Tags `v*`: archive, sign, export and upload to TestFlight.
- `workflow_dispatch` on a tag: as a tag push. On a branch: the version is `0.0.0-dev.<sha>`, and the
  signed .ipa is a workflow artifact. Nothing goes to TestFlight.
- Pull requests that change Apple packaging: an unsigned archive, as a build check. Pull requests never
  get the signing secrets.
- Without the signing secrets the workflow archives unsigned and warns. Nothing is uploaded.
- No OAuth secret: the iOS app has no Google or Microsoft sign-in yet.

Local release from a Mac signed in to Xcode with the team:

```sh
apps/mobile/apple/release.sh 1.2.3 --upload     # or without --upload: export an .ipa to apps/mobile/apple/build/export
apps/mobile/apple/release.sh 1.2.3 --unsigned   # build check, no Apple account needed
```

Then test the build in TestFlight on an iPhone and an iPad, and submit it for review in App Store Connect.

### App Store files

- `apps/mobile/apple/PrivacyInfo.xcprivacy`: no tracking and no collected data. Required-reason APIs:
  UserDefaults (`CA92.1`, `@AppStorage`) and file timestamps (`C617.1`, `stat`/`fstat`/`lstat` from Rust
  and SQLite on files in the app container). The file says how it was measured. Measure again when a
  dependency changes: `nm -u` on the archived binary.
- `Info.plist` (from `project.yml`): `ITSAppUsesNonExemptEncryption` is `false` (see below).
  `NSPhotoLibraryAddUsageDescription` covers "Save to Photos" in the attachment share sheet and on mail
  images. Notifications need no usage string. The app asks for no photos, files, contacts, camera,
  microphone or location access.
- The launch screen is the `LaunchBackground` colour, the same as `Theme.background` (`#FFFFFF` light,
  `#1E1E1E` dark), so the first frame does not flash.
- `brand/store/app-store/`: name, subtitle, promotional text, description, keywords, URLs, age rating
  answers, App Privacy answers and the screenshot shot list.

### Export compliance

The app uses TLS (rustls with ring) for IMAP, SMTP and HTTPS to the user's own servers, and the iOS
Keychain for the password. It does not encrypt data of its own. That is standard encryption for
communication in a mass-market app with public source, which is why `ITSAppUsesNonExemptEncryption` is
`false`. Not verified against today's App Store Connect questionnaire: because rustls is not Apple's own
TLS, App Store Connect can ask about "standard encryption algorithms instead of Apple's" and about
France. If it asks for documents, set the key to `true` and add the `ITSEncryptionExportComplianceCode`
it gives you.

### GitHub secrets for iOS

| Secret                        | Content                                                                                        |
| ----------------------------- | ---------------------------------------------------------------------------------------------- |
| `IOS_SIGNING_P12`             | Base64 of one .p12 with the "Apple Development" and "Apple Distribution" certificates and keys |
| `IOS_SIGNING_P12_PASSWORD`    | The .p12 export password                                                                       |
| `APP_STORE_CONNECT_KEY`       | The full text of the App Store Connect API key file `AuthKey_<id>.p8`                          |
| `APP_STORE_CONNECT_KEY_ID`    | The API key ID (10 characters)                                                                 |
| `APP_STORE_CONNECT_ISSUER_ID` | The issuer ID (a UUID). The same value as `APPLE_NOTARY_ISSUER_ID`.                            |

Xcode creates and downloads the App Store provisioning profile itself with the API key
(`-allowProvisioningUpdates`). No profile secret. Automatic signing archives with the development
certificate and signs the export with the distribution certificate, so the .p12 needs both. Without the
development key Xcode makes a new development certificate on every run, and the team hits Apple's limit.

### Human steps for iOS

Once:

1. In App Store Connect, create the app record for `com.alexkerber.lagom` (name, primary language
   English (U.S.), SKU `lagom-ios`).
2. In the Apple Developer account, make sure the team has an **Apple Development** and an **Apple
   Distribution** certificate, with their private keys in your login keychain. In Keychain Access, select
   both and export them together as `lagom-signing.p12` with a strong password. The development
   profile needs at least one registered device in the team.
3. In App Store Connect, go to Users and Access, Integrations, Team Keys. Create a key with the
   **App Manager** role. Download the .p8 file (Apple allows one download). Note the key ID. Not
   verified that App Manager may create the profile from CI; if step 6 fails on the profile, use **Admin**.
4. Store the secrets:

   ```sh
   base64 -i lagom-signing.p12 | gh secret set IOS_SIGNING_P12
   gh secret set IOS_SIGNING_P12_PASSWORD
   gh secret set APP_STORE_CONNECT_KEY < AuthKey_XXXXXXXXXX.p8
   gh secret set APP_STORE_CONNECT_KEY_ID --body XXXXXXXXXX
   gh secret set APP_STORE_CONNECT_ISSUER_ID --body xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
   ```

5. Delete the local .p12 and .p8 copies, or keep them only in a password manager.
6. Run the workflow with `workflow_dispatch` on a branch. Check that the log signs with "Apple Distribution"
   and exports an .ipa.
7. Put the support and privacy pages on the site (`brand/store/app-store/README.md` lists the URLs).
8. Fill in the listing from `brand/store/app-store/`, the age rating and App Privacy ("Data Not Collected").
   Answer the export compliance question once (see above).

Per release:

- Upload screenshots when the UI changes (6.9" iPhone and 13" iPad, see the shot list).
- Give App Review a test IMAP account and password in the review notes.
- Test the TestFlight build on an iPhone and an iPad, then submit for review.

## Android

Workflow: `.github/workflows/package-android.yml`. Store text and Data safety answers:
`brand/store/google-play/`.

- `build.gradle.kts` reads `LAGOM_VERSION` and sets `versionName` and `versionCode` by the rule above.
  Without it, the build is `0.0.0-dev` with `versionCode` 1. A version that does not fit the rule
  stops the build.
- The release build runs R8 and shrinks resources. `app/proguard-rules.pro` keeps JNA and the UniFFI
  bindings, which find native code by name.
- Tags `v*`: build the App Bundle and the APK, sign them with the upload key and attach both to the
  draft release. Files: `lagom-<LAGOM_VERSION>-android.aab` and `.apk`.
- Pull requests that change Android packaging: build unsigned, as a workflow artifact only.
- `workflow_dispatch`: as a tag, but development version and nothing goes to a release.
- Without the upload key the build is unsigned and the workflow warns. A tag then attaches nothing.

Local release build:

```sh
scripts/build-android.sh
LAGOM_VERSION=1.2.3 apps/mobile/android/gradlew -p apps/mobile/android :app:bundleRelease
```

Signing reads four values from the environment or from `~/.gradle/gradle.properties`, never from the
repository. Without them the release output is unsigned.

| Secret or property                | Content                                                                       |
| --------------------------------- | ----------------------------------------------------------------------------- |
| `LAGOM_ANDROID_KEYSTORE`          | Base64 of the upload keystore (`base64 -i lagom-upload.jks`)                  |
| `LAGOM_ANDROID_KEYSTORE_PASSWORD` | The keystore password                                                         |
| `LAGOM_ANDROID_KEY_ALIAS`         | The key alias (`upload`)                                                      |
| `LAGOM_ANDROID_KEY_PASSWORD`      | The key password. Optional; the keystore password is used when it is missing. |

Per release: download the `.aab` from the draft release and upload it to the Play Console, internal
testing track first, then production.

Human steps (once):

1. Create the Play Console developer account and the app `com.alexkerber.lagom`.
2. Create the upload keystore and store it as GitHub secrets:

   ```sh
   keytool -genkeypair -v -keystore lagom-upload.jks -keyalg RSA -keysize 4096 -validity 10000 -alias upload
   base64 -i lagom-upload.jks | gh secret set LAGOM_ANDROID_KEYSTORE
   gh secret set LAGOM_ANDROID_KEYSTORE_PASSWORD
   gh secret set LAGOM_ANDROID_KEY_ALIAS --body upload
   ```

   Keep the keystore and its password in a password manager, then delete the local copy.

3. Turn on Play App Signing and upload the first `.aab` by hand. Google keeps the app signing key.
4. New personal developer accounts must run a closed test with at least 12 testers for 14 days
   before production access. Plan for this.
5. Fill in the store listing, Data safety, content rating and target audience from
   `brand/store/google-play/`. Icon `brand/store/play-icon-512.png`, feature graphic
   `brand/store/play-feature-1024x500.png`. Take the screenshots from the shot list there.
6. Run the workflow once with `workflow_dispatch` on `main` and check that the log shows the certificate from
   `apksigner verify --print-certs`.

## OAuth providers (all platforms)

- Google: the scope `https://mail.google.com/` is a restricted scope. A public release needs Google's
  OAuth app verification and a yearly CASA security assessment. Until then, only test users added in
  the Google Cloud console can sign in (100 at most). Start this early; it can take weeks.
- Microsoft: complete publisher verification in Microsoft Entra so users do not see "unverified".

## In-app update check: decision

Decision (Alex 2026-11-20): the macOS dmg installs updates itself with Sparkle. See
[Automatic updates on macOS](#automatic-updates-on-macos). Automatic installs matter now. This
replaces the earlier decision of a notify-only check on macOS.

Windows and Linux keep the notify-only plan below. Not implemented yet. Tracked as an open item below.

- The check that fits Windows and Linux:
  - At most once a day, send `GET https://api.github.com/repos/alexkerber/lagom-app/releases/latest` with
    the reqwest client in `lagom-core`. Send no cookies and no identifiers. GitHub sees the IP address
    and a fixed user agent only.
  - Compare `tag_name` with `LAGOM_VERSION` as numbers. Skip the check in development builds.
  - When the release is newer, show one quiet row: "lagom 1.2.4 is available" with a link to the
    release page. No download in the app.
  - Settings get a "Check for updates" switch, on by default, saved with the other settings.
  - The same code serves Windows and Linux. Sparkle updates the macOS dmg. The stores update the
    App Store build, iOS and Android.
- Why it is not on this branch: it needs a settings switch and a row in the main window. Other
  branches change those files now. Do it after they merge.

## Open items

- [ ] Windows and Linux: the update check described above.
