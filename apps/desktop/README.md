# Lagom desktop

macOS, Linux and Windows. This is the first target.

The UI framework is not chosen yet. See the open item in `TODO.md`.

## macOS package

`scripts/package-macos.sh [version]` builds a universal (Apple Silicon and Intel) lagom.app,
signs it with the keychain's Developer ID Application identity (hardened runtime), packs it in a
.dmg, notarizes it and staples the ticket. `.env.oauth` adds the Google and Microsoft client IDs
when present. The result is `target/macos/lagom-<version>.dmg`. CI builds the same dmg on tags;
see `docs/release.md` for the secrets and the full release process.

## Windows installer

`scripts/package-windows.ps1 [version]` (PowerShell 7.3+, Rust MSVC, Inno Setup 6) builds
`target\windows\lagom-<version>-windows-x64-setup.exe`: a per-user install in
`%LOCALAPPDATA%\Programs\lagom`, a Start menu entry, an uninstaller in Settings > Apps, and the
toast identity `app.lagommail.desktop`. It installs WebView2 when missing; without it lagom shows
mail as plain text. The version is the release tag (`docs/release.md`). `WINDOWS_CERTIFICATE` (base64 .pfx) and
`WINDOWS_CERTIFICATE_PASSWORD` sign it; without them it is unsigned. CI builds it in
`.github/workflows/package-windows.yml` and attaches it to the release on `v*` tags.
