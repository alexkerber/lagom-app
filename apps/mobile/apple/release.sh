#!/bin/bash
# Archive the iPhone/iPad app for App Store Connect and export it, or upload it with --upload.
# Signs automatically with the Apple ID signed in to Xcode (team in project.yml). No API key needed.
# Usage: apps/mobile/apple/release.sh [--upload]
set -euo pipefail
cd "$(dirname "$0")"
upload=0
case "${1:-}" in
  "") ;;
  --upload) upload=1 ;;
  *) echo "Usage: $0 [--upload]" >&2; exit 1 ;;
esac

build=build
archive=$build/Lagom.xcarchive
rm -rf "$build"
../../../scripts/build-apple.sh
xcodegen generate --spec project.yml

# The UTC minute as build number: increasing, no file to bump.
# shortcut: two archives in the same minute share a number and App Store Connect rejects the second; use a counter if that starts to happen.
number=$(date -u +%Y%m%d%H%M)
xcodebuild -project Lagom.xcodeproj -scheme Lagom -configuration Release \
  -destination 'generic/platform=iOS' -archivePath "$archive" \
  -allowProvisioningUpdates CURRENT_PROJECT_VERSION="$number" archive

options=ExportOptions.plist
if [ $upload = 1 ]; then
  options=$build/ExportOptions-upload.plist
  cp ExportOptions.plist "$options"
  /usr/libexec/PlistBuddy -c "Set :destination upload" "$options"
fi
xcodebuild -exportArchive -archivePath "$archive" -exportOptionsPlist "$options" \
  -exportPath "$build/export" -allowProvisioningUpdates

if [ $upload = 1 ]; then
  echo "Uploaded build $number. It appears in TestFlight after Apple's processing."
else
  echo "Exported build $number: $(find "$build/export" -name '*.ipa' | head -1)"
fi
