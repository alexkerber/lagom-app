#!/usr/bin/env bash
# Build a universal (Apple Silicon + Intel) lagom.app, sign it, notarize and staple it, and pack it
# in a .dmg. Usage: scripts/package-macos.sh [version]   (version without the "v", e.g. 1.2.3)
# Signing: LAGOM_SIGN_IDENTITY (default "Developer ID Application"); "-" signs ad hoc, which also
# skips notarization. Notarization: an App Store Connect API key (LAGOM_NOTARY_KEY path,
# LAGOM_NOTARY_KEY_ID, LAGOM_NOTARY_ISSUER) or else the notarytool keychain profile
# LAGOM_NOTARY_PROFILE (default lagom-notary). LAGOM_NOTARIZE=0 skips it. See docs/release.md.
# LAGOM_MAS=1 builds for the Mac App Store instead: sandboxed, the iOS bundle ID (one universal
# purchase), and Xcode signs and exports it from an archive (LAGOM_MAS_UPLOAD=1 uploads it). No dmg.
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$repo"
version=${1:-0.1.0}
if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
    echo "Version must look like 1.2.3 or 1.2.3-beta.1, not '$version'." >&2
    exit 2
fi
identity=${LAGOM_SIGN_IDENTITY:-Developer ID Application}
profile=${LAGOM_NOTARY_PROFILE:-lagom-notary}
out=target/macos
app=$out/lagom.app
dmg=$out/lagom-$version.dmg
mas=${LAGOM_MAS:-0}
bundle_id=app.lagommail.desktop
[[ $mas == 0 ]] || bundle_id=com.alexkerber.lagom
# Releases are universal. LAGOM_MAC_TARGETS=aarch64-apple-darwin gives a local build on a Mac without the Intel Rust target.
read -ra targets <<< "${LAGOM_MAC_TARGETS:-aarch64-apple-darwin x86_64-apple-darwin}"

# The single version source for code that needs it (option_env!), see docs/release.md.
export LAGOM_VERSION=$version
# Intel builds otherwise target macOS 10.12; match LSMinimumSystemVersion below.
export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-13.0}
[[ -f .env.oauth ]] || echo 'warning: no .env.oauth, so Google and Microsoft sign-in are off in this build' >&2
for target in "${targets[@]}"; do
    if [[ -f .env.oauth ]]; then
        CARGO_BUILD_TARGET=$target scripts/build-release.sh
    else
        cargo build --release --locked -p lagom --target "$target"
    fi
done

rm -rf "$out"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
binaries=()
for target in "${targets[@]}"; do binaries+=("target/$target/release/lagom"); done
lipo -create -output "$app/Contents/MacOS/lagom" "${binaries[@]}"
[[ -n ${LAGOM_MAC_TARGETS:-} ]] || lipo "$app/Contents/MacOS/lagom" -verify_arch arm64 x86_64
cp brand/app-icon/export/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
# Assets.car carries the dark and tinted icons for macOS 26 (CFBundleIconName); older macOS uses the .icns.
icons=$(mktemp -d)
xcrun actool brand/app-icon/AppIcon.icon --compile "$icons" --platform macosx \
    --minimum-deployment-target "$MACOSX_DEPLOYMENT_TARGET" --app-icon AppIcon --include-all-app-icons \
    --output-partial-info-plist "$icons/partial.plist" >/dev/null
cp "$icons/Assets.car" "$app/Contents/Resources/Assets.car"
rm -rf "$icons"
# The bundle version is the commit time, so rebuilding one commit gives the same Info.plist.
build_number=$(TZ=UTC git log -1 --format=%cd --date=format-local:%Y%m%d%H%M)
year=${build_number:0:4}
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundleName</key><string>lagom</string>
    <key>CFBundleDisplayName</key><string>lagom</string>
    <key>CFBundleIdentifier</key><string>$bundle_id</string>
    <key>CFBundleExecutable</key><string>lagom</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundleIconName</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>${version%%-*}</string>
    <key>CFBundleVersion</key><string>$build_number</string>
    <key>LSMinimumSystemVersion</key><string>$MACOSX_DEPLOYMENT_TARGET</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
    <key>NSHumanReadableCopyright</key><string>© $year Alex Kerber. MIT License.</string>
    <key>NSHighResolutionCapable</key><true/>
    <!-- Lets Intel Macs with two GPUs stay on the integrated one. -->
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
    <!-- Standard TLS only; see "Export compliance" in docs/release.md. -->
    <key>ITSAppUsesNonExemptEncryption</key><false/>
    <!-- mailto links: Settings can make lagom the default mail app. -->
    <key>CFBundleURLTypes</key>
    <array>
        <dict>
            <key>CFBundleURLName</key><string>Email Address URL</string>
            <key>CFBundleURLSchemes</key><array><string>mailto</string></array>
        </dict>
    </array>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null

if [[ $mas != 0 ]]; then
    # Sign locally with the sandbox entitlements; Xcode re-signs for the store and keeps them.
    codesign --force --sign - --entitlements apps/desktop/macos/appstore.entitlements "$app"
    archive=$out/lagom.xcarchive
    mkdir -p "$archive/Products/Applications"
    cp -R "$app" "$archive/Products/Applications/"
    cat > "$archive/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>ArchiveVersion</key><integer>2</integer>
    <key>Name</key><string>lagom</string>
    <key>SchemeName</key><string>lagom</string>
    <key>CreationDate</key><date>$(date -u +%Y-%m-%dT%H:%M:%SZ)</date>
    <key>ApplicationProperties</key>
    <dict>
        <key>ApplicationPath</key><string>Applications/lagom.app</string>
        <key>CFBundleIdentifier</key><string>$bundle_id</string>
        <key>CFBundleShortVersionString</key><string>${version%%-*}</string>
        <key>CFBundleVersion</key><string>$build_number</string>
        <key>Architectures</key><array>$(for a in $(lipo -archs "$app/Contents/MacOS/lagom"); do printf "<string>%s</string>" "$a"; done)</array>
    </dict>
</dict>
</plist>
PLIST
    options=$out/ExportOptions.plist
    cat > "$options" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>method</key><string>app-store-connect</string>
    <key>teamID</key><string>3KFU9JQ5LH</string>
    <key>signingStyle</key><string>automatic</string>
    <key>destination</key><string>$([[ ${LAGOM_MAS_UPLOAD:-0} != 0 ]] && echo upload || echo export)</string>
</dict>
</plist>
PLIST
    auth=(-allowProvisioningUpdates)
    if [[ -n ${LAGOM_ASC_KEY:-} ]]; then
        auth+=(-authenticationKeyPath "$LAGOM_ASC_KEY" -authenticationKeyID "$LAGOM_ASC_KEY_ID"
            -authenticationKeyIssuerID "$LAGOM_ASC_ISSUER")
    fi
    xcodebuild -exportArchive -archivePath "$archive" -exportOptionsPlist "$options" -exportPath "$out/appstore" "${auth[@]}"
    echo "$out/appstore"
    exit 0
fi

# No entitlements file: the app is not sandboxed, and the hardened runtime allows everything it
# uses (network, keychain, notifications, WKWebView, which runs in Apple's own processes).
if [[ $identity == - ]]; then
    codesign --force --options runtime --sign - "$app"
else
    codesign --force --options runtime --timestamp --sign "$identity" "$app"
fi
codesign --verify --strict --deep "$app"

stage=$out/dmg
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
        spctl --assess --type open --context context:primary-signature "$dmg"
    fi
fi
echo "$dmg"
