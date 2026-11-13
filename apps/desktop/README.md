# Lagom desktop

macOS, Linux and Windows. This is the first target.

The UI framework is not chosen yet. See the open item in `TODO.md`.

## macOS package

`scripts/package-macos.sh [version]` builds lagom.app, signs it with the keychain's
Developer ID Application identity (hardened runtime), packs it in a .dmg, notarizes it
with the notarytool keychain profile `lagom-notary` (override with `LAGOM_NOTARY_PROFILE`)
and staples the ticket. `.env.oauth` adds the Google and Microsoft client IDs when present.
The result is `target/macos/lagom-<version>.dmg`. Apple Silicon only for now.

## Windows installer

`scripts/package-windows.ps1` (PowerShell 7.3+, Rust MSVC, Inno Setup 6) builds
`target\windows\lagom-<version>-x64-setup.exe`: a per-user install in
`%LOCALAPPDATA%\Programs\lagom`, a Start menu entry, an uninstaller in Settings > Apps, and the
toast identity `app.lagommail.desktop`. It installs WebView2 when missing; without it lagom shows
mail as plain text. The version is this crate's. `WINDOWS_CERTIFICATE` (base64 .pfx) and
`WINDOWS_CERTIFICATE_PASSWORD` sign it; without them it is unsigned. CI builds it in
`.github/workflows/package-windows.yml` and attaches it to the release on `v*` tags.
