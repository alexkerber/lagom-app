#!/usr/bin/env bash
# Run once on the Mac that signs releases (docs/release.md, "Automatic updates on macOS"). It makes
# the Sparkle EdDSA key pair in the login Keychain, or reuses the one there (account "lagom"),
# stores the private key as the GitHub secret SPARKLE_ED_PRIVATE_KEY and writes the public key to
# apps/desktop/macos/sparkle-ed-public-key. Commit that file: it turns updates on in the dmg.
# The private key only passes through a 0600 temp file that is deleted on exit. It is never printed.
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$repo"
tools=$(scripts/sparkle.sh)/bin
# Its own account, so a Sparkle key of another app never reaches this repository's CI.
account=lagom
# generate_keys keeps an existing key and never overwrites it.
"$tools/generate_keys" --account "$account" >/dev/null
public=$("$tools/generate_keys" --account "$account" -p)
if [[ ! $public =~ ^[A-Za-z0-9+/]{43}=$ ]]; then
    echo "generate_keys did not print an EdDSA public key." >&2
    exit 1
fi
umask 077
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
"$tools/generate_keys" --account "$account" -x "$tmp/key" >/dev/null
gh secret set SPARKLE_ED_PRIVATE_KEY --repo alexkerber/lagom-app < "$tmp/key"
rm -f "$tmp/key"
printf '%s\n' "$public" > apps/desktop/macos/sparkle-ed-public-key
echo "Public key, written to apps/desktop/macos/sparkle-ed-public-key (commit that file):"
echo "$public"
