# Lagom desktop

macOS, Linux and Windows. This is the first target.

The UI framework is not chosen yet. See the open item in `TODO.md`.

## macOS package

`scripts/package-macos.sh [version]` builds a universal (Apple Silicon and Intel) lagom.app,
signs it with the keychain's Developer ID Application identity (hardened runtime), packs it in a
.dmg, notarizes it and staples the ticket. `.env.oauth` adds the Google and Microsoft client IDs
when present. The result is `target/macos/lagom-<version>.dmg`. CI builds the same dmg on tags;
see `docs/release.md` for the secrets and the full release process.
