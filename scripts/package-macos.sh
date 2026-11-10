#!/usr/bin/env bash
# Build lagom.app, sign it with a Developer ID, notarize and staple it, and pack it in a .dmg.
# Usage: scripts/package-macos.sh [version]
# Needs a "Developer ID Application" identity in the keychain and a notarytool keychain profile
# (LAGOM_NOTARY_PROFILE, default lagom-notary). Set LAGOM_NOTARIZE=0 to skip notarization.
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$repo"
version=${1:-0.1.0}
identity=${LAGOM_SIGN_IDENTITY:-Developer ID Application}
profile=${LAGOM_NOTARY_PROFILE:-lagom-notary}
out=target/macos
app=$out/lagom.app
dmg=$out/lagom-$version.dmg

if [[ -f .env.oauth ]]; then
    scripts/build-release.sh
else
    echo 'warning: no .env.oauth, so Google and Microsoft sign-in are off in this build' >&2
    cargo build --release -p lagom
fi

rm -rf "$out"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
# shortcut: Apple Silicon only; build x86_64 too and lipo them when Intel Macs need it.
cp target/release/lagom "$app/Contents/MacOS/lagom"
cp brand/app-icon/export/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>lagom</string>
    <key>CFBundleDisplayName</key><string>lagom</string>
    <key>CFBundleIdentifier</key><string>app.lagommail.desktop</string>
    <key>CFBundleExecutable</key><string>lagom</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$(date -u +%Y%m%d%H%M)</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null

codesign --force --options runtime --timestamp --sign "$identity" "$app"
codesign --verify --strict --deep "$app"

stage=$out/dmg
mkdir -p "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
hdiutil create -quiet -volname lagom -srcfolder "$stage" -format UDZO "$dmg"
rm -rf "$stage"
codesign --force --timestamp --sign "$identity" "$dmg"

if [[ ${LAGOM_NOTARIZE:-1} != 0 ]]; then
    xcrun notarytool submit "$dmg" --keychain-profile "$profile" --wait
    xcrun stapler staple "$dmg"
    spctl --assess --type open --context context:primary-signature "$dmg"
fi
echo "$dmg"
