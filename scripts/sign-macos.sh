#!/usr/bin/env bash
# Sign target/macos/lagom.app (from scripts/package-macos.sh), pack it in a .dmg, notarize and
# staple it. Usage: scripts/sign-macos.sh <version>   (version without the "v", e.g. 1.2.3)
# It runs no cargo and nothing from inside the app, so CI runs it on a fresh runner that gets the
# Developer ID and the notary key and never builds (docs/release.md, "macOS").
# LAGOM_SIGN_IDENTITY, LAGOM_NOTARIZE and LAGOM_NOTARY_*: as in scripts/package-macos.sh.
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$repo"
version=${1:?usage: scripts/sign-macos.sh <version>}
if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
    echo "Version must look like 1.2.3 or 1.2.3-beta.1, not '$version'." >&2
    exit 2
fi
identity=${LAGOM_SIGN_IDENTITY:-Developer ID Application}
profile=${LAGOM_NOTARY_PROFILE:-lagom-notary}
out=target/macos
app=$out/lagom.app
dmg=$out/lagom-$version.dmg
framework=$app/Contents/Frameworks/Sparkle.framework
plist=$app/Contents/Info.plist
key() { /usr/libexec/PlistBuddy -c "Print :$1" "$plist" 2>/dev/null; }

# In CI the app comes from the build job. Check it is the dmg build of this version and this
# commit's Sparkle key. That catches mix-ups; it cannot prove the code inside is benign.
if [[ ! -f $app/Contents/MacOS/lagom || $(key CFBundleIdentifier) != app.lagommail.desktop \
    || $(key CFBundleShortVersionString) != "${version%%-*}" ]]; then
    echo "$app is not the dmg build of lagom $version." >&2
    exit 1
fi
if [[ -s apps/desktop/macos/sparkle-ed-public-key ]]; then
    if [[ ! -d $framework || $(key SUPublicEDKey) != "$(< apps/desktop/macos/sparkle-ed-public-key)" ]]; then
        echo "$app lacks Sparkle or has another SUPublicEDKey than apps/desktop/macos/sparkle-ed-public-key." >&2
        exit 1
    fi
    sparkle=1
elif [[ -e $app/Contents/Frameworks ]] || key SUPublicEDKey >/dev/null; then
    echo "$app has Sparkle, but this commit has no apps/desktop/macos/sparkle-ed-public-key." >&2
    exit 1
else
    sparkle=0
fi

# No entitlements file: the app is not sandboxed, and the hardened runtime allows everything it
# uses (network, keychain, notifications, WKWebView, which runs in Apple's own processes).
sign() {
    if [[ $identity == - ]]; then
        codesign --force --options runtime --sign - "$@"
    else
        codesign --force --options runtime --timestamp --sign "$identity" "$@"
    fi
}
if [[ $sparkle == 1 ]]; then
    # Inside out, as Sparkle documents for signing without Xcode.
    sign "$framework/Versions/B/Autoupdate"
    sign "$framework/Versions/B/Updater.app"
    sign "$framework"
fi
sign "$app"
codesign --verify --strict --deep "$app"

stage=$out/dmg
rm -rf "$stage" "$dmg"
mkdir -p "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
hdiutil create -quiet -volname lagom -srcfolder "$stage" -format UDZO "$dmg"
rm -rf "$stage"

if [[ $identity != - ]]; then
    codesign --force --timestamp --sign "$identity" "$dmg"
    if [[ ${LAGOM_NOTARIZE:-1} != 0 ]]; then
        if [[ -n ${LAGOM_NOTARY_KEY:-} ]]; then
            xcrun notarytool submit "$dmg" --key "$LAGOM_NOTARY_KEY" \
                --key-id "$LAGOM_NOTARY_KEY_ID" --issuer "$LAGOM_NOTARY_ISSUER" --wait
        else
            xcrun notarytool submit "$dmg" --keychain-profile "$profile" --wait
        fi
        xcrun stapler staple "$dmg"
        xcrun stapler validate "$dmg"
        spctl --assess --type open --context context:primary-signature "$dmg"
        # A local release signs the update with the Keychain key here. CI signs in its own job on a
        # fresh runner, so the private key never shares a machine with cargo and build scripts.
        if [[ $sparkle == 1 && -z ${CI:-} ]]; then scripts/sparkle-item.sh "$version" >&2; fi
    fi
fi
echo "$dmg"
