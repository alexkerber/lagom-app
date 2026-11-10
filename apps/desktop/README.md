# Lagom desktop

macOS, Linux and Windows. This is the first target.

The UI framework is not chosen yet. See the open item in `TODO.md`.

## macOS package

`scripts/package-macos.sh [version]` builds Lagom.app, signs it with the keychain's
Developer ID Application identity (hardened runtime), packs it in a .dmg, notarizes it
with the notarytool keychain profile `lagom-notary` (override with `LAGOM_NOTARY_PROFILE`)
and staples the ticket. `.env.oauth` adds the Google and Microsoft client IDs when present.
The result is `target/macos/Lagom-<version>.dmg`. Apple Silicon only for now.
