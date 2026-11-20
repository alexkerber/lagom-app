#!/usr/bin/env bash
# Usage: scripts/sparkle-item.sh <version>   (after scripts/package-macos.sh <version> notarized the dmg)
# Signs target/macos/lagom-<version>.dmg with the Sparkle EdDSA key and writes its appcast <item>
# to target/macos/lagom-<version>.appcast.xml, then prints that path. Attach the file to the
# release next to the dmg; scripts/appcast.sh builds the feed from the published releases.
# Key: the SPARKLE_ED_PRIVATE_KEY environment variable (the CI secret), or else the login Keychain
# (scripts/sparkle-keys.sh). Only a notarized dmg of a Sparkle build gets an item.
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$repo"
version=${1:?usage: scripts/sparkle-item.sh <version>}
if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
    echo "Version must look like 1.2.3 or 1.2.3-beta.1, not '$version'." >&2
    exit 2
fi
dmg=target/macos/lagom-$version.dmg
item=target/macos/lagom-$version.appcast.xml
xcrun stapler validate "$dmg" >/dev/null
# The item describes the app inside this dmg, so read its Info.plist from there.
mnt=$(mktemp -d)
trap 'hdiutil detach -quiet "$mnt" 2>/dev/null || true; rmdir "$mnt" 2>/dev/null || true' EXIT
hdiutil attach -quiet -readonly -nobrowse -mountpoint "$mnt" "$dmg"
plist=$mnt/lagom.app/Contents/Info.plist
key() { /usr/libexec/PlistBuddy -c "Print :$1" "$plist"; }
if [[ $(key CFBundleShortVersionString) != "${version%%-*}" ]]; then
    echo "$dmg does not hold version $version." >&2
    exit 1
fi
if ! key SUPublicEDKey >/dev/null 2>&1; then
    echo "This dmg was built without Sparkle (no SUPublicEDKey), so it gets no appcast item." >&2
    exit 1
fi

tools=$(scripts/sparkle.sh)/bin
if [[ -n ${SPARKLE_ED_PRIVATE_KEY:-} ]]; then
    signature=$(printf '%s' "$SPARKLE_ED_PRIVATE_KEY" | "$tools/sign_update" --ed-key-file - "$dmg")
else
    signature=$("$tools/sign_update" --account lagom "$dmg")
fi
if [[ ! $signature =~ ^sparkle:edSignature=\"[A-Za-z0-9+/=]+\"\ length=\"[0-9]+\"$ ]]; then
    echo "sign_update gave no EdDSA signature." >&2
    exit 1
fi
release=https://github.com/alexkerber/lagom-app/releases
cat > "$item" <<XML
        <item>
            <title>lagom $version</title>
            <pubDate>$(LC_ALL=C date -u '+%a, %d %b %Y %H:%M:%S +0000')</pubDate>
            <sparkle:version>$(key CFBundleVersion)</sparkle:version>
            <sparkle:shortVersionString>$version</sparkle:shortVersionString>
            <sparkle:minimumSystemVersion>$(key LSMinimumSystemVersion)</sparkle:minimumSystemVersion>
            <sparkle:releaseNotesLink>$release/tag/v$version</sparkle:releaseNotesLink>
            <enclosure url="$release/download/v$version/lagom-$version.dmg" type="application/octet-stream" $signature/>
        </item>
XML
echo "$item"
