#!/bin/bash
# Archive the iPhone/iPad app for App Store Connect and export it, or upload it with --upload.
# --unsigned archives without signing and stops there: a build check without an Apple account.
# Signs automatically with the Apple ID signed in to Xcode (team in project.yml), or with an
# App Store Connect API key when LAGOM_ASC_KEY (path to the .p8), LAGOM_ASC_KEY_ID and
# LAGOM_ASC_ISSUER are set, as in CI.
# Usage: apps/mobile/apple/release.sh <version> [--upload | --unsigned]
set -euo pipefail
cd "$(dirname "$0")"
usage="Usage: $0 <version, e.g. 1.2.3 or 1.2.3-beta.4> [--upload | --unsigned]"
version=${1:-}
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || { echo "$usage" >&2; exit 1; }
mode=${2:-export}
case "$mode" in
  export | --upload | --unsigned) ;;
  *) echo "$usage" >&2; exit 1 ;;
esac

build=build
archive=$build/Lagom.xcarchive
rm -rf "$build"
../../../scripts/build-apple.sh
xcodegen generate --spec project.yml

# CFBundleShortVersionString takes three numbers only, so a pre-release ships as its 1.2.3 part.
# The UTC minute as build number: increasing, no file to bump.
# shortcut: two archives in the same minute share a number and App Store Connect rejects the second; use a counter if that starts to happen.
number=$(date -u +%Y%m%d%H%M)
versions=(MARKETING_VERSION="${version%%-*}" CURRENT_PROJECT_VERSION="$number")

if [ "$mode" = --unsigned ]; then
  xcodebuild -project Lagom.xcodeproj -scheme Lagom -configuration Release \
    -destination 'generic/platform=iOS' -archivePath "$archive" \
    CODE_SIGNING_ALLOWED=NO "${versions[@]}" archive
  echo "Archived unsigned build $number of $version: $archive"
  exit 0
fi

auth=(-allowProvisioningUpdates)
if [ -n "${LAGOM_ASC_KEY:-}" ]; then
  auth+=(-authenticationKeyPath "$LAGOM_ASC_KEY" -authenticationKeyID "$LAGOM_ASC_KEY_ID"
    -authenticationKeyIssuerID "$LAGOM_ASC_ISSUER")
fi
xcodebuild -project Lagom.xcodeproj -scheme Lagom -configuration Release \
  -destination 'generic/platform=iOS' -archivePath "$archive" \
  "${auth[@]}" "${versions[@]}" archive

options=ExportOptions.plist
if [ "$mode" = --upload ]; then
  options=$build/ExportOptions-upload.plist
  cp ExportOptions.plist "$options"
  /usr/libexec/PlistBuddy -c "Set :destination upload" "$options"
fi
xcodebuild -exportArchive -archivePath "$archive" -exportOptionsPlist "$options" \
  -exportPath "$build/export" "${auth[@]}"

if [ "$mode" = --upload ]; then
  echo "Uploaded build $number of $version. It appears in TestFlight after Apple's processing."
else
  echo "Exported build $number of $version: $(find "$build/export" -name '*.ipa' | head -1)"
fi
